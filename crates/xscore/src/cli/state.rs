// cli/state.rs (v0.6.0 — schema v3 migrations, ADR 0010 D8)
//
// `xscore state migrate` has two jobs, told apart by the version on disk:
//
//   * v2 → v3: add the session tier (one session per link, a `session_id` on
//     every child).
//   * v3 → v3: upgrade a v3 file written before the control client became the
//     only tmux path — old tmux links become tmux sessions and their rows are
//     dropped. The version stays 3 (v3 was never released), which is exactly
//     why this step has to exist: a migrated-but-old file must not silently
//     lose the user's tmux session.
//
// Both run WITHOUT hydrating the manager: hydration would hit the very gate
// these commands exist to clear.
use clap::Subcommand;

#[derive(Subcommand)]
pub enum StateCmd {
    /// Convert state.json in place: a v2 file gains the session tier (schema
    /// v3), and a v3 file from the old tmux path is upgraded.
    Migrate,
}

pub fn run(cmd: StateCmd) -> anyhow::Result<()> {
    match cmd {
        StateCmd::Migrate => {
            let version = crate::core::state::on_disk_version()?;
            match version {
                2 => {
                    let created = crate::core::state::migrate_v2_to_v3()?;
                    println!("migrated state.json to schema v3: {created} session(s) created");
                }
                3 => {
                    let converted = crate::core::state::migrate_v3_tmux_links()?;
                    println!("upgraded state.json: {converted} tmux session(s) converted");
                }
                other => anyhow::bail!(
                    "state.json schema_version={other}; `state migrate` converts a v2 file or \
                     upgrades an older v3 file"
                ),
            }
        }
    }
    Ok(())
}
