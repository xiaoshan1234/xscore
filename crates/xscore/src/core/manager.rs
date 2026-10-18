// Placeholder. Filled in by task-06 (Link / Window / Pane structs + Registries + SessionManager).
// The 3-tier model (ADR 0006) means:
//   manager.rs      -> SessionManager with add_link / open_link / close_link / del_link, add_window / del_window, add_pane / del_pane
//   link.rs         -> Link struct (transport + sink + windows list)
//   pane.rs         -> PaneInfo struct (formerly Pane struct)
//   link_registry.rs -> LinkRegistry (by_id, by_address)
//   pane_registry.rs -> PaneRegistry (pane_id -> PaneInfo)
