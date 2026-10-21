// cli/state.rs (v0.6.0 — schema v3 migration, ADR 0010 D8)
//
// `xscore state migrate` converts a v2 `state.json` in place. Loading a v2 file
// is a hard error (`parse_snapshot`), so this is the one explicit way forward —
// the same policy v0.4.0 used for v0.3.0 files.
//
// It runs WITHOUT hydrating the manager: hydration would hit the very version
// gate this command exists to clear.
use clap::Subcommand;

#[derive(Subcommand)]
pub enum StateCmd {
    /// Convert a v2 state.json to schema v3: adds the session tier, one
    /// session per link, and a session_id on every link, window and pane.
    Migrate,
}

pub fn run(cmd: StateCmd) -> anyhow::Result<()> {
    match cmd {
        StateCmd::Migrate => {
            let created = crate::core::state::migrate_v2_to_v3()?;
            println!(
                "migrated state.json to schema v3: {created} session(s) created"
            );
        }
    }
    Ok(())
}
