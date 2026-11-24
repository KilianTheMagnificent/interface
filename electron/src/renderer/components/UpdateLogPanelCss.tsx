// The update log's sheet, kept apart from the panel so Settings can include it
// while the panel (and the update history it reads) loads only when Settings ->
// Updates opens. A .tsx so the component-sheet scans in
// theme-accessibility.test.ts still read it.
export const UPDATE_LOG_PANEL_CSS = `
.settings-update-log { display:grid; gap:10px; margin-top:18px; min-width:0; }
.settings-update-log-head {
  display:flex; flex-wrap:wrap; align-items:baseline; justify-content:space-between; gap:8px;
}
.settings-update-log-title {
  margin:0; color:var(--fg-strong); font-size:var(--text-body); font-weight:var(--wght-strong); line-height:var(--lh-ui);
}
.settings-update-log-summary { color:var(--fg-dim); font-size:var(--text-meta); }
.settings-update-log-controls {
  display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px;
}
.settings-update-log-tabs, .settings-update-log-kinds {
  display:flex; flex-wrap:wrap; align-items:center; gap:4px;
}
.settings-update-log-pill {
  display:inline-flex; align-items:center; gap:5px; height:26px; padding:0 9px;
  color:var(--fg-muted); background:var(--surface); border:var(--border-w) solid var(--border);
  border-radius:var(--radius-pill); font:inherit; font-size:var(--text-meta); cursor:pointer;
}
.settings-update-log-pill:hover { color:var(--fg-strong); background:var(--row-hover); }
.settings-update-log-pill[aria-pressed="true"] {
  color:var(--fg-strong); background:var(--surface-raised); border-color:var(--border-strong);
  font-weight:var(--wght-strong);
}
.settings-update-log-search {
  flex:1 1 200px; min-width:160px; height:var(--control-h); padding:0 9px;
  color:var(--fg); background:var(--surface); border:var(--border-w) solid var(--border);
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-meta);
}
.settings-update-log-search::placeholder { color:var(--fg-faint); }
.settings-update-log-list {
  display:grid; gap:8px; max-height:520px; overflow-y:auto; padding-right:2px;
}
.settings-update-log-card {
  min-width:0; padding:12px 14px; background:var(--surface); border:var(--border-w) solid var(--border);
  border-radius:var(--radius-overlay);
}
.settings-update-log-card-head {
  display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px;
}
.settings-update-log-card-title {
  color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong); line-height:var(--lh-ui);
}
.settings-update-log-meta {
  display:inline-flex; flex-wrap:wrap; align-items:center; gap:6px; color:var(--fg-dim); font-size:var(--text-meta);
}
.settings-update-log-badge {
  display:inline-flex; align-items:center; padding:1px 6px; color:var(--fg-muted);
  background:var(--surface-raised); border-radius:var(--radius-pill); font-size:var(--text-meta); line-height:var(--lh-ui);
}
.settings-update-log-badge.is-current {
  color:var(--fg-strong); font-weight:var(--wght-strong);
}
.settings-update-log-code {
  padding:1px 5px; color:var(--fg-muted); background:var(--editor-bg);
  border-radius:var(--radius-control); font-family:var(--font-mono); font-size:var(--text-meta);
}
.settings-update-log-notes {
  margin:6px 0 0; color:var(--fg-muted); font-size:var(--text-meta); line-height:var(--lh-ui);
  overflow-wrap:anywhere;
}
.settings-update-log-pins {
  margin:5px 0 0; color:var(--fg-dim); font-family:var(--font-mono); font-size:var(--text-meta);
  line-height:var(--lh-ui); overflow-wrap:anywhere;
}
.settings-update-log-items {
  display:grid; gap:8px; margin:10px 0 0; padding:10px 0 0; list-style:none;
  border-top:var(--border-w) solid var(--border);
}
.settings-update-log-item { min-width:0; }
.settings-update-log-item + .settings-update-log-item {
  padding-top:8px; border-top:var(--border-w) solid var(--border);
}
.settings-update-log-item-head {
  display:flex; flex-wrap:wrap; align-items:center; gap:6px; min-width:0;
}
.settings-update-log-item-name {
  color:var(--fg-strong); font-size:var(--text-meta); font-weight:var(--wght-strong);
}
.settings-update-log-more { justify-self:center; margin-top:4px; }
`;
