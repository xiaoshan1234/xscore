// core/pane.rs — pure Pane UI metadata
//
// `PaneInfo` contains only viewport geometry, window placement and an optional
// Buffer reference. Bytes, source Links and tmux handles belong to BufferInfo
// and the backend SessionManager.
//
// The registry and manager keep UI metadata separate from runtime bridges.
