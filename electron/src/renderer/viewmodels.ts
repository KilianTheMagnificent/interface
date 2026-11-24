/**
 * Sidebar, session-activity, session-config and usage view models, one module
 * per rule set. This barrel keeps the names every caller and suite imported
 * from here; new code imports the module that owns the rule.
 */
export {
  attentionAnnouncement, attentionSessions, formatTurnElapsed, needsAttention, pendingApproval,
  rememberTurnStart, sessionActivity, stampTurnClock, TURN_CLOCK_MIN_MS, turnProgress,
} from "./session-activity";
export type { TurnProgress } from "./session-activity";
export {
  configInForce, engineCan, engineOpsMap, fastModeInForce, permissionPicker,
  withoutDefaultPermissionMode,
} from "./session-config-view";
export type {
  ConfigInForce, EngineOpMap, FastModeInForce, PermissionPicker,
} from "./session-config-view";
export {
  buildSidebarGroups, buildSidebarView, compareSidebarGroups, importProgressLabel, importSummary,
  managedWorktreeName, OTHER_PROJECT, OTHER_PROJECT_HINT, OTHER_PROJECT_NAME, queryTerms,
  sessionAgeLabel, sessionMatches, sessionSurfaceOf, sidebarEmptyState, sidebarFilterLabel,
  sidebarRowMeta, terminalHasActiveWork, TRIVIAL_SESSION_BYTES, TRIVIAL_SESSION_KIB,
  workspacesActiveFirst, workspaceSidebarBadge, workspaceSidebarDone, workspaceSidebarPips,
  workspaceSidebarProjectPath, workspaceSidebarTitle,
} from "./sidebar-view";
export type {
  ConversationSurface, EmptyAction, EmptyState, SidebarGroup, SidebarRow, SidebarRowMeta,
  SidebarTerminalProjection, SidebarTerminalRow, SidebarView,
} from "./sidebar-view";
export {
  barWidth, buildUsageView, formatReset, USAGE_STALE_MS, usageEnginePercent,
  usageFreshness, usageRailReading, usageStripItems, usageStripSummary, usageStripTone,
} from "./usage-view";
export type {
  UsageEngineRef, UsageEngineView, UsageFreshness, UsageStripItem, UsageStripTone,
  UsageWindowView,
} from "./usage-view";
