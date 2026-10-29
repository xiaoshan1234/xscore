use super::*;
use crate::infra::link::NoopTransport;
use crate::infra::tmux::{TmuxControl, TmuxHostConfig, TmuxSocket};
use crate::service::link_service::{EndpointRef, Link, LinkId};
use crate::service::session_service::service::{control_link_address, TmuxTarget, TmuxView};
use crate::service::session_service::{SessionBackend, SessionId, SessionInfo};
use crate::{Error, Result};
use std::sync::Arc;

impl SessionService {
    pub(super) async fn close_tmux_control(&self, id: SessionId) {
        match &self.daemon {
            Some(client) => {
                let _ = client.close_tmux(id).await;
            }
            None => {
                if let Some(control) = self.tmux.take_control(id).await {
                    let _ = control.close().await;
                }
            }
        }
    }

    pub async fn create_tmux_session(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        cols: u16,
        rows: u16,
    ) -> Result<SessionId> {
        self.create_tmux_session_with(name, host, socket, cols, rows, TmuxCreateOptions::default())
            .await
    }

    /// `create_tmux_session` plus the first pane's command and directory.
    pub async fn create_tmux_session_with(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        cols: u16,
        rows: u16,
        opts: TmuxCreateOptions,
    ) -> Result<SessionId> {
        self.require_free_session_name(name).await?;
        // `has-session` exits 1 when the session is absent — that is the answer
        // we want, not an error.
        if self.tmux_session_exists(&host, &socket, name).await? {
            return Err(Error::TmuxSessionExists(name.to_string()));
        }

        let mut args: Vec<String> = vec![
            "new-session".into(),
            "-d".into(),
            "-s".into(),
            name.into(),
            "-x".into(),
            cols.to_string(),
            "-y".into(),
            rows.to_string(),
        ];
        if let Some(cwd) = opts.cwd.as_deref().filter(|c| !c.is_empty()) {
            args.push("-c".into());
            args.push(cwd.to_string());
        }
        if let Some(command) = opts.command.as_deref().filter(|c| !c.trim().is_empty()) {
            args.extend([
                "--".into(),
                "/bin/sh".into(),
                "-c".into(),
                command.to_string(),
            ]);
        }
        let out = self.run_tmux(&host, &socket, &args).await?;
        if !out.status.success() {
            return Err(self.tmux_failure(&host, name, "new-session", &out));
        }
        self.finish_tmux_session(name, host, socket, true).await
    }

    /// Take over a tmux session that already exists (ADR 0008, now through the
    /// control client instead of one exec per command).
    pub async fn adopt_tmux_session(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
    ) -> Result<SessionId> {
        self.require_free_session_name(name).await?;
        if !self.tmux_session_exists(&host, &socket, name).await? {
            return Err(Error::TmuxTargetGone {
                host: crate::infra::tmux::host_label(&host),
                session: name.to_string(),
            });
        }
        // Adoption never creates and never kills: the session was the user's
        // before it was ours.
        self.finish_tmux_session(name, host, socket, false).await
    }

    /// Re-read a tmux session's structure now.
    ///
    /// Listings call this because a CLI invocation is one process per command:
    /// between commands nothing of ours is listening, so a window created
    /// outside xscore is invisible until something asks. In daemon mode that is
    /// one `Structure` request to the resident client; in-process it is an
    /// attach for the lifetime of this command (arch.md §7).
    pub async fn refresh_tmux_session(&self, id: SessionId) -> Result<()> {
        // A session whose rows came from `state migrate` has no control link
        // yet; reconciliation records every pane against it.
        self.ensure_control_link(id).await?;
        match &self.daemon {
            Some(client) => match client.structure(id).await {
                Ok(structure) => {
                    let view = TmuxView::from_structure(structure);
                    self.reconcile_all(id, &view, None).await.map(|_| ())
                }
                // The daemon holds no client yet (it restarted, or this is the
                // first command since boot). Opening one enumerates as well,
                // so both paths end in the same rows.
                Err(_) => self.attach_daemon(id).await,
            },
            None => {
                let control = self.ensure_local_control(id).await?;
                let view = self.view_with(&control, id).await?;
                self.reconcile_all(id, &view, Some(&control))
                    .await
                    .map(|_| ())
            }
        }
    }

    /// The session's control client **in this process**, attached on demand.
    ///
    /// Only used when there is no daemon (`data_smoke`, an embedding caller).
    /// With a daemon the client is the daemon's (`OpenTmux`), so this is never
    /// reached — `attach_daemon` is its counterpart.
    ///
    /// Attaching is also the reconnect path: after `%exit` (the client died,
    /// the ssh channel dropped) the handle is closed, so it is dropped and
    /// replaced. Output produced while nothing was attached is gone — tmux
    /// replays nothing (ADR 0010 §D2) — but the session itself keeps running.
    pub(super) async fn ensure_local_control(&self, id: SessionId) -> Result<Arc<TmuxControl>> {
        // Held across the check-and-spawn by TmuxRuntime.
        let _guard = self.tmux.attach_guard().await;
        let existing = { self.tmux.control(id).await };
        if let Some(control) = existing {
            if !control.is_closed() {
                return Ok(control);
            }
            self.tmux.take_control(id).await;
        }
        self.attach_local_control(id).await
    }

    /// Spawn the in-process control client, enumerate the session, start the
    /// pumps.
    ///
    /// The caller serialises attaches through TmuxRuntime; this is not reentrant.
    /// A session that has no control link yet (one whose rows were converted by
    /// `state migrate`) gets it here: attaching IS what creates it.
    pub(super) async fn attach_local_control(&self, id: SessionId) -> Result<Arc<TmuxControl>> {
        // Row first: `sync_window` records every pane against it.
        self.ensure_control_link(id).await?;
        let target = self.tmux_target(id).await?;
        let prefix = crate::infra::tmux::host_prefix(&target.host);
        let control = Arc::new(TmuxControl::spawn(&prefix, &target.socket, &target.name)?);

        // Subscribe BEFORE enumerating: `%output` is not replayed, so a pane
        // added between the enumeration and the subscription would stream into
        // nothing until it next prints.
        let events = control.subscribe();

        let view = match TmuxView::enumerate(&control, &target.name).await {
            Ok(view) => view,
            Err(e) => {
                // Do not leave a client attached to a session we could not adopt.
                let _ = control.close().await;
                // The client dying during the handshake means one of two things
                // — the session is gone, or the host never answered — and
                // `attach` cannot tell them apart (only a one-shot's exit 255
                // can, arch.md §6). Say what happened instead of surfacing the
                // raw "closed".
                return Err(Error::Sink(format!(
                    "could not attach to tmux session `{}` on {}: {e}",
                    target.name,
                    crate::infra::tmux::host_label(&target.host)
                )));
            }
        };

        if let Err(e) = self.reconcile_all(id, &view, Some(&control)).await {
            let _ = control.close().await;
            return Err(e);
        }

        self.tmux.put_control(id, Arc::clone(&control)).await;
        self.record_control(id, &control).await;
        self.start_event_pump(id, Arc::clone(&control), events);
        Ok(control)
    }

    /// Attach in daemon mode: one `OpenTmux` (create-or-reuse the client) that
    /// answers with the structure, so the rows come from the same round trip
    /// that opened the session.
    pub(super) async fn attach_daemon(&self, id: SessionId) -> Result<()> {
        // Row first: `sync_window` records every pane against it.
        self.ensure_control_link(id).await?;
        let target = self.tmux_target(id).await?;
        let Some(client) = &self.daemon else {
            return Err(Error::Daemon("no daemon configured".into()));
        };
        let structure = client
            .open_tmux(
                id,
                target.host,
                target.socket,
                &target.name,
                target.cols,
                target.rows,
            )
            .await?;
        let view = TmuxView::from_structure(structure);
        self.reconcile_all(id, &view, None).await?;
        Ok(())
    }

    /// Enumerate a tmux session through a client this process holds.
    pub(super) async fn view_with(
        &self,
        control: &Arc<TmuxControl>,
        id: SessionId,
    ) -> Result<TmuxView> {
        let target = self.tmux_target(id).await?;
        TmuxView::enumerate(control, &target.name).await
    }

    /// Note the live client on the session's control link (ADR 0010 §4: the
    /// link IS the connection, so the handle belongs on the link row as well as
    /// in the session-keyed map). Best effort: a session that has no control
    /// link yet (attachment happens before the row exists on the very first
    /// attach) simply has nothing to record.
    pub(super) async fn record_control(&self, id: SessionId, control: &Arc<TmuxControl>) {
        let Some(link_id) = self.control_link_id(id).await else {
            return;
        };
        let Some(existing) = self.link_service.get(link_id).await else {
            return;
        };
        let rebuilt = Link {
            id: link_id,
            info: existing.info.clone(),
            transport: Arc::clone(&existing.transport),
            control: Some(Arc::clone(control)),
            connected: true,
        };
        self.link_service.insert(rebuilt).await;
    }

    /// The session's one link (ADR 0010 §4, "One tmux session, one link").
    pub(super) async fn control_link_id(&self, id: SessionId) -> Option<LinkId> {
        self.link_service
            .list()
            .await
            .into_iter()
            .find(|l| l.info.session_id == id && l.info.is_control())
            .map(|l| l.id)
    }

    pub(super) async fn require_free_session_name(&self, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(Error::Invalid("session name must not be empty".into()));
        }
        if self.sessions.get_by_name(name).await.is_some() {
            return Err(Error::SessionExists(name.to_string()));
        }
        Ok(())
    }

    /// Shared tail of create/adopt: session row, control link, attach, save.
    pub(super) async fn finish_tmux_session(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        created: bool,
    ) -> Result<SessionId> {
        let sid = self
            .add_session(SessionInfo::new_tmux(
                name,
                host.clone(),
                socket.clone(),
                name,
            ))
            .await?;

        let outcome = async {
            self.open_control_link(sid).await?;
            self.save_state().await
        }
        .await;
        if let Err(e) = outcome {
            // Rows first, so nothing points at a session that is about to go.
            // An adopted session is left running: the user's tmux was there
            // before us, and a failed *adoption* must not destroy it.
            let _ = self.detach_session(sid).await;
            if created {
                let _ = self
                    .run_tmux(
                        &host,
                        &socket,
                        &[
                            "kill-session".to_string(),
                            "-t".to_string(),
                            name.to_string(),
                        ],
                    )
                    .await;
            }
            return Err(e);
        }
        Ok(sid)
    }

    /// Create the session's single link and attach its control client — in the
    /// daemon when there is one, here otherwise.
    pub(super) async fn open_control_link(&self, id: SessionId) -> Result<LinkId> {
        let link_id = self.ensure_control_link(id).await?;
        match &self.daemon {
            Some(_) => self.attach_daemon(id).await?,
            None => {
                self.ensure_local_control(id).await?;
            }
        }
        Ok(link_id)
    }

    /// Make sure the session has its control link **row**, and return its id.
    ///
    /// Idempotent: this is the one place that materialises the link, so a
    /// session created by `session new --tmux`, one adopted, and one whose rows
    /// came from `state migrate` all end up with exactly one. Attaching is a
    /// separate step (`open_control_link` / `ensure_local_control` /
    /// `attach_daemon`): a row is metadata, a client is a process.
    pub(super) async fn ensure_control_link(&self, id: SessionId) -> Result<LinkId> {
        if let Some(link_id) = self.control_link_id(id).await {
            return Ok(link_id);
        }
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux {
            host,
            socket,
            session,
        } = info.backend
        else {
            return Err(Error::NotTmuxSession(id));
        };
        let link_id = LinkId::new();
        // The link is the connection to the *server*, so the process is the
        // connection: `address` records how the tmux protocol is carried (a
        // local `tmux` process, or an `ssh` invocation), while the live client
        // is the `TmuxControl` handle in `control`. Neither a transport nor a
        // sink is ever used to move these bytes — the endpoint says so.
        let prefix = crate::infra::tmux::host_prefix(&host);
        let argv = crate::infra::tmux::argv(&prefix, &socket, &session);
        let address = control_link_address(&host, &argv[prefix.len()..]);

        let link = Link {
            id: link_id,
            info: crate::service::link_service::LinkInfo {
                id: link_id,
                session_id: id,
                name: format!("{session}-ctl"),
                created_at: chrono::Utc::now(),
                address,
                endpoint: EndpointRef::Control,
            },
            transport: Arc::new(NoopTransport),
            control: None,
            connected: true,
        };
        self.link_service.insert(link).await;
        Ok(link_id)
    }

    // ----- enumeration -----

    /// Where a tmux session lives, plus the size the caller saw.
    ///
    /// The daemon needs this for `OpenTmux` (it never reads `state.json`); the
    /// size is informational there (`-C attach` takes none).
    pub(super) async fn tmux_target(&self, id: SessionId) -> Result<TmuxTarget> {
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux {
            host,
            socket,
            session,
        } = info.backend
        else {
            return Err(Error::NotTmuxSession(id));
        };
        let size = self
            .ui
            .windows()
            .await
            .iter()
            .find(|w| w.info.session_id == id)
            .map(|w| (w.info.cols, w.info.rows))
            .unwrap_or((80, 24));
        Ok(TmuxTarget {
            host,
            socket,
            name: session,
            cols: size.0,
            rows: size.1,
        })
    }
}
