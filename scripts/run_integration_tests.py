#!/usr/bin/env python3
"""Run xscore integration tests and write JSON, JUnit XML and Markdown reports."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
RESULT_LINE = re.compile(
    r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;"
)


@dataclass(frozen=True)
class Suite:
    name: str
    command: list[str]


def utc_timestamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def parse_config(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    contents = path.read_text(encoding="utf-8-sig")

    for line_number, raw_line in enumerate(contents.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise ValueError(f"{path}:{line_number}: expected key=value")
        key, value = (part.strip() for part in line.split("=", 1))
        if key not in {"ssh_host", "ssh_password", "ssh_port"}:
            raise ValueError(f"{path}:{line_number}: unknown key {key!r}")
        if key in values:
            raise ValueError(f"{path}:{line_number}: duplicate key {key!r}")
        if not value:
            raise ValueError(f"{path}:{line_number}: {key!r} must not be empty")
        values[key] = value

    for required in ("ssh_host", "ssh_password"):
        if required not in values:
            raise ValueError(f"{path}: missing required key {required!r}")
    if "@" not in values["ssh_host"]:
        raise ValueError(f"{path}: ssh_host must use user@host format")

    port = int(values.get("ssh_port", "22"))
    if not 1 <= port <= 65535:
        raise ValueError(f"{path}: ssh_port must be between 1 and 65535")
    values["ssh_port"] = str(port)
    return values


def target_directory() -> Path:
    output = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return Path(json.loads(output.stdout)["target_directory"])


def redact(text: str, secret: str) -> str:
    return text.replace(secret, "***") if secret else text


def run_suite(
    suite: Suite,
    env: dict[str, str],
    logs_dir: Path,
    timeout_seconds: int,
    secret: str,
) -> dict[str, Any]:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            suite.command,
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout_seconds,
        )
        output = completed.stdout + completed.stderr
        exit_code = completed.returncode
        status = "passed" if exit_code == 0 else "failed"
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or ""
        stderr = error.stderr or ""
        if isinstance(stdout, bytes):
            stdout = stdout.decode("utf-8", errors="replace")
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", errors="replace")
        output = stdout + stderr
        exit_code = None
        status = "timeout"

    sanitized = redact(output, secret)
    log_path = logs_dir / f"{suite.name}.log"
    log_path.write_text(sanitized, encoding="utf-8")

    passed = failed = ignored = 0
    for passed_text, failed_text, ignored_text in RESULT_LINE.findall(sanitized):
        passed += int(passed_text)
        failed += int(failed_text)
        ignored += int(ignored_text)

    return {
        "name": suite.name,
        "command": suite.command,
        "status": status,
        "exit_code": exit_code,
        "duration_seconds": round(time.monotonic() - started, 3),
        "tests_passed": passed,
        "tests_failed": failed,
        "tests_ignored": ignored,
        "log": str(log_path.relative_to(ROOT)),
    }


def write_junit(path: Path, suites: list[dict[str, Any]]) -> None:
    root = ET.Element(
        "testsuites",
        tests=str(len(suites)),
        failures=str(sum(suite["status"] != "passed" for suite in suites)),
        time=f"{sum(suite['duration_seconds'] for suite in suites):.3f}",
    )
    for suite in suites:
        case = ET.SubElement(
            root,
            "testsuite",
            name=suite["name"],
            tests="1",
            failures=str(int(suite["status"] != "passed")),
            skipped="0",
            time=f"{suite['duration_seconds']:.3f}",
        )
        testcase = ET.SubElement(
            case,
            "testcase",
            classname="xscore.integration",
            name=suite["name"],
            time=f"{suite['duration_seconds']:.3f}",
        )
        if suite["status"] != "passed":
            failure = ET.SubElement(
                testcase,
                "failure",
                message=f"integration suite {suite['status']}",
            )
            failure.text = f"See {suite['log']}"
        ET.SubElement(testcase, "system-out").text = f"See {suite['log']}"

    ET.ElementTree(root).write(path, encoding="utf-8", xml_declaration=True)


def write_markdown(path: Path, result: dict[str, Any]) -> None:
    suites = result["suites"]
    lines = [
        "# xscore integration test report",
        "",
        f"- Started: {result['started_at']}",
        f"- Finished: {result['finished_at']}",
        f"- Result: **{result['status']}**",
        "",
        "| Suite | Status | Passed | Failed | Ignored | Duration | Log |",
        "|---|---:|---:|---:|---:|---:|---|",
    ]
    for suite in suites:
        lines.append(
            f"| {suite['name']} | {suite['status']} | {suite['tests_passed']} | "
            f"{suite['tests_failed']} | {suite['tests_ignored']} | "
            f"{suite['duration_seconds']:.3f}s | `{suite['log']}` |"
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=ROOT / "test.config")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=int, default=180)
    args = parser.parse_args()

    output_dir = (args.output or ROOT / "test-results" / utc_timestamp()).resolve()
    logs_dir = output_dir / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)

    started_at = datetime.now(timezone.utc).isoformat()
    status = "passed"
    suites: list[dict[str, Any]] = []

    try:
        config = parse_config(args.config.resolve())
    except (OSError, ValueError) as error:
        status = "failed"
        suites.append(
            {
                "name": "config",
                "command": [],
                "status": "failed",
                "exit_code": None,
                "duration_seconds": 0.0,
                "tests_passed": 0,
                "tests_failed": 1,
                "tests_ignored": 0,
                "log": str(error),
            }
        )
        config = {"ssh_password": ""}

    env = os.environ.copy()
    for key in ("XSCORE_SSH_HOST", "XSCORE_SSH_PASSWORD", "PASSWD"):
        env.pop(key, None)
    env["CARGO_TERM_COLOR"] = "never"

    if not suites:
        target = target_directory()
        askpass = target / "debug" / ("xs.exe" if os.name == "nt" else "xs")
        build = run_suite(
            Suite("build", ["cargo", "build", "-p", "emulator-frontend"]),
            env,
            logs_dir,
            args.timeout,
            config["ssh_password"],
        )
        suites.append(build)

        if build["status"] == "passed":
            env["XSCORE_TEST_ASKPASS"] = str(askpass)
            planned = [
                Suite(
                    "local-domain",
                    ["cargo", "test", "-p", "emulator", "--test", "local_domain", "--", "--nocapture"],
                ),
                Suite(
                    "ssh-shell",
                    ["cargo", "test", "-p", "emulator", "--test", "ssh_shell", "--", "--ignored", "--nocapture"],
                ),
                Suite(
                    "tmux-emulator",
                    ["cargo", "test", "-p", "emulator", "--test", "tmux_ssh", "--", "--ignored", "--nocapture"],
                ),
                Suite(
                    "tmux-frontend",
                    ["cargo", "test", "-p", "emulator-frontend", "--test", "tmux_ssh", "--", "--ignored", "--nocapture"],
                ),
            ]
            for suite in planned:
                result = run_suite(
                    suite, env, logs_dir, args.timeout, config["ssh_password"]
                )
                suites.append(result)
                if result["status"] != "passed":
                    status = "failed"

    if any(suite["status"] != "passed" for suite in suites):
        status = "failed"

    result = {
        "schema_version": 1,
        "status": status,
        "started_at": started_at,
        "finished_at": datetime.now(timezone.utc).isoformat(),
        "config": {
            "path": str(args.config.resolve()),
            "ssh_credentials_configured": bool(config.get("ssh_password")),
        },
        "suites": suites,
    }

    (output_dir / "results.json").write_text(
        json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    write_junit(output_dir / "junit.xml", result["suites"])
    write_markdown(output_dir / "summary.md", result)

    print(f"integration tests: {status}")
    for suite in suites:
        print(
            f"  {suite['name']}: {suite['status']} "
            f"({suite['duration_seconds']:.3f}s)"
        )
    print(f"report: {output_dir / 'summary.md'}")
    return 0 if status == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
