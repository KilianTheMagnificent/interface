import { FitAddon } from '@xterm/addon-fit';
import { Profiler, type ReactNode } from 'react';
import { flushSync } from 'react-dom';
import type { Root } from 'react-dom/client';
import type { TerminalWorkspaceActivityRequest, TerminalSummary, TerminalWorkspaceRecord } from '../../shared/ipc';
import type { TerminalMenuModel, TerminalWorkspaceSwitcher } from '../components/TerminalWorkspace';
import { IntegratedTerminalCenter, type TerminalRendererApi } from '../components/IntegratedTerminalCenter';
import { BrowserPane, type BrowserPaneProps } from '../components/BrowserPane';
import { TerminalPane, type TerminalPaneProps } from '../components/TerminalPane';
import { TerminalView, type TerminalViewProps } from '../components/TerminalView';
import { attachTerminalsBalanced, EMPTY_TERMINAL_WORKSPACE_STATE,
  serializeTerminalWorkspaceState } from '../terminal-workspace';
import { TERMINAL_ANNOUNCEMENT_LIMIT, TERMINAL_ANNOUNCEMENT_NOTICE } from '../terminal-announcements';
import { assert, frames, noop, wait } from './fixture-support';
import { appendTerminalExcerpt } from '../composer-quote';
import { createDraftStore } from '../draft-store';

/** Real xterm and center events, synthetic provider bytes; never opens a live PTY. */
export async function terminalInteractionFixture(root: Root): Promise<Record<string, unknown>> {
  const stamp = '2026-09-11T00:00:00Z';
  const terminals: TerminalSummary[] = Array.from({ length: 4 }, (_, index) => ({
    id: `00000000-0000-4000-8000-${String(index + 701).padStart(12, '0')}`,
    generation: 1, title: `Interaction ${index}`, root: '/fixture/interaction', cwd: '/fixture/interaction',
    state: 'running', size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: 'shell' }, createdAt: stamp, updatedAt: stamp,
  }));
  const record: TerminalWorkspaceRecord = { id: 'interaction-workspace', name: 'Interaction',
    createdAt: stamp, updatedAt: stamp, terminals,
    layout: serializeTerminalWorkspaceState(attachTerminalsBalanced(EMPTY_TERMINAL_WORKSPACE_STATE,
      terminals.map(t => t.id), i => `interaction-pane-${i}`, i => `interaction-split-${i}`)) };
  const otherRecord: TerminalWorkspaceRecord = { ...record, id: 'sleep-test-workspace', name: 'Sleep test', terminals: [], layout: serializeTerminalWorkspaceState(EMPTY_TERMINAL_WORKSPACE_STATE) };
  let switcher: TerminalWorkspaceSwitcher | null = null;
  const workspaceActivity: TerminalWorkspaceActivityRequest[] = [];
  let receive: Parameters<TerminalRendererApi['onTerminalData']>[0] | undefined;
  let lifecycle: Parameters<TerminalRendererApi['onTerminalLifecycle']>[0] | undefined;
  let revision = 1;
  let commits = 0;
  let fits = 0;
  let outstanding = 0;
  const errors: string[] = [];
  const inputs: Array<{ id: string; text: string; at: number }> = [];
  const encoder = new TextEncoder();
  const emit = (id: string, text: string): void => {
    outstanding++;
    receive!({ kind: 'data', terminalId: id, generation: 1, sequence: '0', data: encoder.encode(text) },
      () => { outstanding--; });
  };
  const unexpected = (): never => { throw new Error('Interaction fixture attempted an unrelated operation'); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision, terminals }), listTerminalLaunchProfiles: async () => [],
    getTerminalWorkspaces: async () => [record, otherRecord],
    setTerminalWorkspaceActivity: async request => { workspaceActivity.push(request); return terminals.find(t => t.id === request.id)!; }, setTerminalWorkspaces: async value => value,
    onTerminalData: callback => { receive = callback; return noop; },
    onTerminalLifecycle: callback => { lifecycle = callback; return noop; }, onTerminalDataFailure: () => noop,
    sendTerminalInput: (id, _generation, bytes) => {
      const text = new TextDecoder().decode(bytes);
      inputs.push({ id, text, at: performance.now() });
      if (text === 'x') emit(id, 'x');
    },
    resizeTerminal: async request => ({ ...terminals.find(t => t.id === request.id)!, size: request.size }),
    createTerminal: unexpected, closeTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  const fit = FitAddon.prototype.fit;
  FitAddon.prototype.fit = function () { fits++; fit.call(this); };
  // Count renders per emulator and per pane without touching production code:
  // React reads a forwardRef's `render` and a memo's `type` off these objects.
  const viewRenders = new Map<string, number>();
  const paneRenders = new Map<string, number>();
  const viewType = TerminalView as unknown as { render: (props: TerminalViewProps, ref: unknown) => ReactNode };
  const paneType = TerminalPane as unknown as { type: (props: TerminalPaneProps) => ReactNode };
  const renderView = viewType.render;
  const renderPane = paneType.type;
  viewType.render = (props, ref) => {
    viewRenders.set(props.terminalId, (viewRenders.get(props.terminalId) ?? 0) + 1);
    return renderView(props, ref);
  };
  paneType.type = props => {
    paneRenders.set(props.pane.id, (paneRenders.get(props.pane.id) ?? 0) + 1);
    return renderPane(props);
  };
  const browserType = BrowserPane as unknown as { type: (props: BrowserPaneProps) => ReactNode };
  const renderBrowser = browserType.type;
  let browserRenders = 0;
  browserType.type = props => { browserRenders++; return renderBrowser(props); };
  const rendered = (counts: Map<string, number>, keys: readonly string[]): number =>
    keys.reduce((sum, key) => sum + (counts.get(key) ?? 0), 0);
  let menu: TerminalMenuModel | null = null;
  const until = async (ready: () => boolean, message: string): Promise<void> => {
    const deadline = performance.now() + 4000;
    while (!ready()) { assert(performance.now() < deadline, message); await wait(10); }
  };
  const view = (index: number): HTMLElement => {
    const element = document.querySelector<HTMLElement>(`[data-terminal-id="${terminals[index].id}:1"]`);
    assert(element, `Missing terminal ${index}`); return element;
  };
  const pane = (index: number): HTMLElement => view(index).closest<HTMLElement>('.terminal-pane')!;
  let output: ReturnType<typeof setInterval> | undefined;
  // T3 section 9: "Send selection to thread…" writes the thread's draft as
  // App does (appendTerminalExcerpt into the draft store).
  const threadDrafts = createDraftStore({});
  const sentSelections: Array<{ key: string; title: string; selection: string }> = [];
  const feedbackTargets = [{ key: 'claude#review', title: 'Review thread' }];
  const addTerminalSelection = (key: string, title: string, selection: string): boolean => {
    sentSelections.push({ key, title, selection });
    const write = appendTerminalExcerpt(threadDrafts.get(key), title, selection);
    if (write === null) return false;
    threadDrafts.set(key, write.text);
    return true;
  };
  try {
    flushSync(() => root.render(<Profiler id="terminal-interaction" onRender={() => { commits++; }}>
      <div style={{ height: 640, display: 'flex' }}><IntegratedTerminalCenter api={api}
        activeCwd="/fixture/interaction" visible onSwitcherChange={value => { switcher = value; }} onError={error => errors.push(error)}
        outputFeedbackTargets={feedbackTargets} onAddTerminalSelection={addTerminalSelection}
        onMenuChange={value => { menu = value; }} />
      </div></Profiler>));
    await until(() => document.querySelectorAll('.terminal-view[data-terminal-ready]').length === 4,
      'Interaction terminals did not finish setup');
    // Cursor-only redraws do not trip xterm's line-feed announcement limit.
    // Exercise this BEFORE history/typing/focus clears or caps the live region.
    const liveRegion = (index: number): HTMLElement => {
      const region = view(index).querySelector<HTMLElement>('.xterm-accessibility .live-region');
      assert(region, `Missing accessible announcement region ${index}`); return region;
    };
    const redraw = '\x1b[H⠁⠈ ⢀⠐ ⠄⠠ ⡀⠂'.repeat(256);
    const announced = new Set<number>();
    let maxAnnouncementLength = 0;
    let maxAnimationFrameMs = 0;
    for (let round = 0; round < 80; round++) {
      const started = performance.now();
      for (const terminal of terminals) emit(terminal.id, redraw);
      await until(() => outstanding === 0, 'Cursor animation writes did not drain');
      await frames();
      maxAnimationFrameMs = Math.max(maxAnimationFrameMs, performance.now() - started);
      terminals.forEach((_, index) => {
        const region = liveRegion(index);
        const text = region.textContent ?? '';
        maxAnnouncementLength = Math.max(maxAnnouncementLength, text.length);
        assert(text.length <= TERMINAL_ANNOUNCEMENT_LIMIT, `Announcement grew to ${text.length} characters`);
        if (text.startsWith(TERMINAL_ANNOUNCEMENT_NOTICE)) announced.add(index);
        assert(getComputedStyle(region).whiteSpace === 'pre', 'Announcement text still wraps into a 1px box');
      });
    }
    await until(() => terminals.every((_, index) =>
      liveRegion(index).textContent?.startsWith(TERMINAL_ANNOUNCEMENT_NOTICE)),
    'Cursor animations never exercised the announcement bound in all four panes');
    assert(announced.size === 4, 'Some panes never received sustained accessible output');
    // Drain xterm's trailing one-second announcement debounce before testing
    // input clear; already-buffered animation text can otherwise arrive later.
    for (const terminal of terminals) emit(terminal.id, '\x1b[Hannouncement-settled');
    await until(() => terminals.every((_, index) =>
      liveRegion(index).textContent?.endsWith('announcement-settled')),
    'Trailing animation announcements did not drain');
    // A very large single update must be clipped at the mutation checkpoint,
    // including characterData writes, without changing the emulator's text.
    const beforeOversized = view(0).querySelector('.xterm-accessibility-tree')?.textContent;
    const region = liveRegion(0);
    region.firstChild!.textContent = '⠁⠈ ⢀⠐ '.repeat(30_000);
    await new Promise<void>(resolve => queueMicrotask(resolve));
    assert(region.textContent!.length <= TERMINAL_ANNOUNCEMENT_LIMIT, 'One oversized announcement escaped the bound');
    assert(view(0).querySelector('.xterm-accessibility-tree')?.textContent === beforeOversized,
      'Clipping announcements changed accessible terminal rows');
    // Input still clears announcement history and subsequent ordinary output
    // is announced without a stale truncation notice.
    {
      const input = view(0).querySelector('textarea')!;
      input.dispatchEvent(new KeyboardEvent('keypress', {
        key: 'x', charCode: 120, keyCode: 120, bubbles: true, cancelable: true,
      }));
      assert(region.textContent === '', 'Input did not clear announcement history');
      emit(terminals[0].id, '\r\nApproval needed: continue?\r\n');
      await until(() => region.textContent?.includes('Approval needed: continue?') === true,
        'Ordinary announcements did not resume after input');
      assert(!region.textContent?.includes(TERMINAL_ANNOUNCEMENT_NOTICE), 'Truncation notice survived input clear');
      assert(region.getAttribute('aria-live') === 'assertive', 'Announcements lost their accessibility role');
    }
    // Distinct rows make the accessibility comparison observe scrolling even
    // when the latest marker has not entered the rendered viewport yet.
    const history = Array.from({ length: 5200 }, (_, index) => `retained history line ${index}\r\n`).join('');
    for (const terminal of terminals) emit(terminal.id, history);
    await until(() => outstanding === 0, 'History writes did not drain');
    await wait(250); await frames();
    const originals = terminals.map((_, index) => view(index));
    flushSync(() => pane(0).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })));
    await frames(); await frames();
    const initialCommits = commits;
    for (let index = 0; index < 12; index++) {
      flushSync(() => pane(0).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })));
      await frames();
    }
    const repeatedFocusCommits = commits - initialCommits;
    assert(repeatedFocusCommits === 0, `Repeated focus caused ${repeatedFocusCommits} React commits`);
    const initialFits = fits;
    viewRenders.clear(); paneRenders.clear();
    for (let index = 0; index < 8; index++) {
      lifecycle!({ revision: ++revision, kind: 'upsert', terminal: { ...terminals[3], title: `Background ${index}` } });
      await frames();
    }
    const backgroundTitleFits = fits - initialFits;
    assert(backgroundTitleFits === 0, `Background titles caused ${backgroundTitleFits} focused-terminal fits`);
    // A center render costs what changed: one terminal's title, activity or
    // pane focus redraws its own pane and emulator, never the others'.
    const streamOf = (index: number): string => `${terminals[index].id}:1`;
    const paneIdOf = (index: number): string => pane(index).dataset.terminalPaneId!;
    const others = (index: number): number[] => terminals.map((_, at) => at).filter(at => at !== index);
    const otherViews = (index: number): number => rendered(viewRenders, others(index).map(streamOf));
    const otherPanes = (index: number): number => rendered(paneRenders, others(index).map(paneIdOf));
    // The title loop above ran with the counters live.
    const backgroundTitleViewRenders = otherViews(3);
    const backgroundTitlePaneRenders = otherPanes(3);
    assert(backgroundTitleViewRenders === 0 && backgroundTitlePaneRenders === 0,
      `Background titles redrew ${backgroundTitleViewRenders} other emulators and ${backgroundTitlePaneRenders} other panes`);
    // An echo equal to the row already held (a resize reply and its push)
    // moves only the revision fence: nothing renders.
    let before = commits;
    lifecycle!({ revision: ++revision, kind: 'upsert', terminal: { ...terminals[3], title: 'Background 7' } });
    await frames();
    const equalEchoCommits = commits - before;
    assert(equalEchoCommits === 0, `An unchanged lifecycle echo caused ${equalEchoCommits} React commits`);
    // Background activity: a shell turns working on output and idle when quiet.
    const status = (index: number): string => document.querySelector(
      `.terminal-tab[data-terminal-id="${terminals[index].id}"] .terminal-tab-status`)?.className ?? '';
    await until(() => !status(3).includes('is-working'), 'Background shell never went quiet');
    viewRenders.clear(); paneRenders.clear();
    emit(terminals[3].id, 'background work\r\n');
    await until(() => status(3).includes('is-working'), 'Background output never showed as work');
    await frames();
    const activityViewRenders = otherViews(3);
    const activityPaneRenders = otherPanes(3);
    assert(activityViewRenders === 0 && activityPaneRenders === 0,
      `A background activity flip redrew ${activityViewRenders} other emulators and ${activityPaneRenders} other panes`);
    // Moving focus redraws the two panes whose ring moves, and no emulator.
    viewRenders.clear(); paneRenders.clear();
    flushSync(() => pane(1).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })));
    await frames();
    const focusMovePaneRenders = rendered(paneRenders, [2, 3].map(paneIdOf));
    const focusMoveViewRenders = rendered(viewRenders, terminals.map((_, index) => streamOf(index)));
    assert(focusMovePaneRenders === 0 && focusMoveViewRenders === 0,
      `Moving focus redrew ${focusMovePaneRenders} uninvolved panes and ${focusMoveViewRenders} emulators`);
    assert(rendered(paneRenders, [0, 1].map(paneIdOf)) > 0, 'Moving focus did not redraw the focused panes');
    flushSync(() => pane(0).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })));
    await frames();
    // Normal scrollback remains native. Read the accessibility tree instead of
    // xterm private fields to prove that the visible rows actually moved.
    const visibleText = (): string => view(2).querySelector('.xterm-accessibility-tree')?.textContent ?? '';
    emit(terminals[2].id, 'latest marker\r\n');
    await until(() => outstanding === 0, 'Scroll marker did not drain');
    // xterm debounces accessibility updates for one second. Wait for the
    // marker so a delayed output render cannot masquerade as a scroll.
    await until(() => visibleText().includes('latest marker'), 'Scroll marker was not rendered');
    const beforeScroll = visibleText();
    const firstVisibleRow = (): number => Number(view(2).querySelector('.xterm-accessibility-tree')
      ?.firstElementChild?.getAttribute('aria-posinset'));
    const beforeRow = firstVisibleRow();
    const screen = view(2).querySelector('.xterm-screen')!;
    const bounds = screen.getBoundingClientRect();
    // Chromium's synthetic event exposes wheelDeltaY with deltaY's sign.
    // xterm reads that legacy field first, so provide native wheel signs:
    // negative deltaY / positive wheelDeltaY both mean scrolling upward.
    const wheelEvent = new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaMode: 0,
      deltaY: -240, clientX: bounds.left + 30, clientY: bounds.top + 30 });
    Object.defineProperties(wheelEvent, { wheelDeltaY: { value: 240 },
      wheelDeltaX: { value: 0 }, wheelDelta: { value: 240 } });
    screen.dispatchEvent(wheelEvent);
    await until(() => firstVisibleRow() < beforeRow && visibleText() !== beforeScroll,
      'Normal scrollback did not move upward');
    assert(!inputs.some(input => input.id === terminals[2].id), 'Normal wheel emitted provider input');
    // Keep output flowing while switching panes and sending actual xterm key events.
    output = setInterval(() => {
      for (const terminal of terminals) emit(terminal.id, '\x1b[0moutput '.repeat(256) + '\r\n');
    }, 8);
    const inputLatency: number[] = [];
    const switchLatency: number[] = [];
    for (let turn = 0; turn < 16; turn++) {
      const index = turn % terminals.length;
      const started = performance.now();
      view(index).querySelector('.xterm-screen')!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
      await until(() => view(index).contains(document.activeElement), 'Pane switch did not focus its emulator');
      switchLatency.push(performance.now() - started);
      const inputAt = performance.now();
      const prior = inputs.length;
      view(index).querySelector('textarea')!.dispatchEvent(new KeyboardEvent('keypress', {
        key: 'x', charCode: 120, keyCode: 120, bubbles: true, cancelable: true,
      }));
      await until(() => inputs.length > prior, 'Keystroke did not reach terminal input');
      const received = inputs[inputs.length - 1];
      assert(received.id === terminals[index].id && received.text === 'x', 'Keystroke crossed pane identity');
      inputLatency.push(received.at - inputAt);
      await wait(10);
    }
    clearInterval(output); output = undefined;
    await until(() => outstanding === 0, 'Output credits did not drain');
    assert(terminals.every((_, index) => view(index) === originals[index]), 'Interaction remounted a terminal');
    const latestActive = (id: string): boolean | undefined => workspaceActivity.filter(request => request.id === id).at(-1)?.active;
    await until(() => terminals.every(terminal => latestActive(terminal.id) === true), 'Active workspace never reached the host');
    for (const terminal of terminals) emit(terminal.id, '\r\nsleep-resume-marker\r\n');
    await until(() => outstanding === 0, 'Final output did not drain');
    flushSync(() => switcher!.onSelect(otherRecord.id));
    await until(() => terminals.every(terminal => latestActive(terminal.id) === false), 'Leaving a workspace did not mark its terminals inactive');
    assert(workspaceActivity.filter(request => !request.active).every(request => request.screen !== undefined && request.outputCursor !== undefined), 'Workspace switch lost its exact rendered prompt observation');
    flushSync(() => switcher!.onSelect(record.id));
    await until(() => terminals.every(terminal => latestActive(terminal.id) === true), 'Returning did not wake the existing generations');
    await until(() => document.querySelectorAll('.terminal-view[data-terminal-ready]').length === 4, 'Woken workspace did not render all terminals');
    await until(() => terminals.every((_, index) => view(index).querySelector('.xterm-accessibility-tree')?.textContent?.includes('sleep-resume-marker')), 'Workspace wake lost the retained output');
    assert(workspaceActivity.every(request => request.generation === 1), 'Workspace wake restarted a generation');
    // Splitter frames: leave the tidy grid for draggable splits, then move one.
    // Last, because changing the arrangement re-slots every pane and drops focus.
    assert(menu !== null, 'The layout menu was never published');
    flushSync(() => menu!.onTidy());
    await frames();
    const splitter = document.querySelector<HTMLElement>('.terminal-splitter');
    assert(splitter, 'Leaving the tidy grid drew no splitter');
    viewRenders.clear(); paneRenders.clear();
    before = commits;
    const keySplitter = (key: string): void => {
      flushSync(() => splitter.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true })));
    };
    keySplitter('End');
    await frames();
    const splitterFrameCommits = commits - before;
    const splitterViewRenders = rendered(viewRenders, terminals.map((_, index) => streamOf(index)));
    const splitterPaneRenders = rendered(paneRenders, terminals.map((_, index) => paneIdOf(index)));
    assert(splitterViewRenders === 0 && splitterPaneRenders === 0,
      `A splitter frame redrew ${splitterViewRenders} emulators and ${splitterPaneRenders} panes`);
    // Held against the bound, the same ratio again renders nothing at all.
    before = commits;
    keySplitter('End');
    await frames();
    const heldSplitterCommits = commits - before;
    assert(heldSplitterCommits === 0, `A splitter held at its bound caused ${heldSplitterCommits} React commits`);
    flushSync(() => menu!.onTidy());
    await frames();
    // An open Browser has nothing to redraw when another terminal changes.
    const newPane = pane(0).querySelector<HTMLButtonElement>('[aria-label="New terminal in this pane"]');
    assert(newPane, 'Missing the pane menu that opens a Browser');
    flushSync(() => newPane.click());
    const browserItem = (): HTMLButtonElement | undefined => [...document.querySelectorAll<HTMLButtonElement>(
      '[role="menu"][aria-label="Open in a new pane"] [role="menuitem"]')]
      .find(item => item.textContent?.trim() === 'Browser');
    await until(() => browserItem() !== undefined, 'The pane menu offered no Browser');
    const sourcePaneId = paneIdOf(0);
    const paneBoxes = (): string => JSON.stringify([...document.querySelectorAll<HTMLElement>('.terminal-pane')]
      .map(element => [element.dataset.terminalPaneId, ...Object.values(element.getBoundingClientRect().toJSON())]));
    const boxesBefore = paneBoxes();
    flushSync(() => browserItem()!.click());
    // The Browser is a tab in the pane whose menu opened it (owner,
    // 2026-11-15): no pane is added and none moves or resizes.
    const browserPane = (): HTMLElement | null =>
      document.querySelector<HTMLElement>('.browser-pane')?.closest<HTMLElement>('.terminal-pane') ?? null;
    // It opens on Google with its address bar ready for a search.
    await until(() => browserPane() !== null && document.querySelector('.browser-page-view') !== null,
      'The Browser never opened its home page');
    await frames();
    assert(document.querySelectorAll('.terminal-pane').length === 4, 'The Browser opened a pane instead of a tab');
    assert(browserPane()!.dataset.terminalPaneId === sourcePaneId
      && browserPane()!.getAttribute('aria-label') === 'Terminal pane, 1 terminal, 1 page',
      `The Browser tab joined ${browserPane()!.getAttribute('aria-label') ?? 'no pane'} instead of its source pane`);
    assert(paneBoxes() === boxesBefore, 'Opening the Browser moved or resized a pane');
    assert(browserPane()!.querySelector('.terminal-tab[data-terminal-id^="browser:"]')
      ?.getAttribute('aria-selected') === 'true', 'The Browser tab is not the one its pane shows');
    const address = browserPane()!.querySelector<HTMLInputElement>('[aria-label="Address"]');
    assert(address && document.activeElement === address, 'The Browser lost focus to the dismissed menu');
    assert(address.value === 'https://www.google.com/' && address.placeholder === 'Search Google or type a web address',
      `The Browser opened on ${address.value} instead of Google`);
    assert(!browserPane()!.querySelector('.browser-pane-notice'), 'The Browser showed a notice over its home page');
    assert(!document.querySelector('[role="dialog"]'), 'The Browser opened a dialog instead of a pane');
    // Its page lives in the centre's page layer, anchored over the pane's
    // slot, and stays the same element while the layout moves the pane.
    const page = document.querySelector<HTMLElement>('.terminal-center-split > .browser-page-layer > .browser-page');
    const slot = (): HTMLElement | null => browserPane()?.querySelector<HTMLElement>('.browser-pane-slot') ?? null;
    const anchor = page?.style.getPropertyValue('position-anchor') ?? '';
    assert(page && anchor.startsWith('--browser-page-') && slot()?.style.getPropertyValue('anchor-name') === anchor
      && !page.classList.contains('is-parked'), 'The Browser page was not anchored over its pane');
    assert(!page.classList.contains('is-covered') && page.querySelector('.browser-page-view') !== null,
      'The Browser page did not show its home page');
    // While the page shows, the terminal-only actions step aside; the
    // terminal tab beside it brings them back and parks the page.
    const moreItems = async (): Promise<string[]> => {
      const more = browserPane()!.querySelector<HTMLButtonElement>('button[aria-label="More pane actions"]')!;
      flushSync(() => more.click());
      await until(() => document.querySelector('[role="menu"][aria-label="Pane actions"]') !== null,
        'The ··· menu did not open');
      const items = [...document.querySelectorAll('[role="menu"][aria-label="Pane actions"] [role="menuitem"]')]
        .map(item => item.textContent?.trim() ?? '');
      flushSync(() => more.click());
      await frames();
      return items;
    };
    const pageItems = await moreItems();
    assert(!pageItems.includes('Clear scrollback') && !pageItems.some(item => item.startsWith('Send selection')),
      `The ··· menu offered terminal actions over a page: ${pageItems.join(', ')}`);
    assert(browserPane()!.querySelector('[aria-label="Close active terminal (ends it)"]') === null,
      'The pane head offered to end a terminal over a page');
    const terminalTab = browserPane()!.querySelector<HTMLButtonElement>(
      `.terminal-tab[data-terminal-id="${terminals[0].id}"]`);
    assert(terminalTab, 'The mixed pane lost its terminal tab');
    flushSync(() => terminalTab.click());
    await until(() => page.classList.contains('is-parked'), 'The page stayed over the terminal tab');
    assert((await moreItems()).includes('Clear scrollback'), 'The terminal tab did not bring Clear scrollback back');
    const pageTab = browserPane()!.querySelector<HTMLButtonElement>('.terminal-tab[data-terminal-id^="browser:"]')!;
    flushSync(() => pageTab.click());
    await until(() => !page.classList.contains('is-parked'), 'The page did not come back with its tab');
    flushSync(() => menu!.onTidy());
    await frames();
    assert(document.querySelector('.browser-page') === page && slot()?.style.getPropertyValue('anchor-name') === anchor,
      'Rearranging the panes replaced the Browser page');
    flushSync(() => menu!.onTidy());
    await frames();
    // A second page in the same pane: the background tab's page parks out of
    // sight instead of escaping its pane, and comes back when shown again.
    const browserMenu = browserPane()!.querySelector<HTMLButtonElement>('[aria-label="New terminal in this pane"]');
    flushSync(() => browserMenu!.click());
    const newTabItem = (): HTMLButtonElement | undefined => [...document.querySelectorAll<HTMLButtonElement>(
      '[role="menu"][aria-label="Open in a new pane"] [role="menuitem"]')]
      .find(item => item.textContent?.trim().startsWith('New tab'));
    await until(() => newTabItem() !== undefined, 'The Browser pane menu offered no New tab');
    flushSync(() => newTabItem()!.click());
    await until(() => document.querySelectorAll('.browser-page').length === 2, 'New tab did not open a second page');
    await frames();
    assert(page.classList.contains('is-parked')
      && document.querySelector('.terminal-pane[aria-label="Terminal pane, 1 terminal, 2 pages"]') !== null
      && document.querySelectorAll('.terminal-pane .terminal-tab-title').length === 6,
      'The background Browser tab did not park its page');
    const secondClose = [...document.querySelectorAll<HTMLButtonElement>('.terminal-tab-wrap.is-active .terminal-tab-close')]
      .find(button => button.getAttribute('aria-label') === 'Close Browser');
    assert(secondClose, 'Missing the second Browser tab close');
    flushSync(() => secondClose.click());
    await until(() => document.querySelectorAll('.browser-page').length === 1 && !page.classList.contains('is-parked'),
      'The first Browser page did not come back with its tab');
    browserRenders = 0;
    for (let index = 0; index < 4; index++) {
      lifecycle!({ revision: ++revision, kind: 'upsert', terminal: { ...terminals[3], title: `Browsing ${index}` } });
      await frames();
    }
    const backgroundTitleBrowserRenders = browserRenders;
    assert(backgroundTitleBrowserRenders === 0,
      `Background titles redrew the open Browser ${backgroundTitleBrowserRenders} times`);
    const closeBrowser = browserPane()?.querySelector<HTMLButtonElement>('[aria-label="Close browser"]');
    assert(closeBrowser, 'Missing the Browser close');
    flushSync(() => closeBrowser.click());
    await until(() => !document.querySelector('.browser-pane') && !document.querySelector('.browser-page'),
      'The Browser stayed open after Close');
    assert(document.querySelectorAll('.terminal-pane').length === 4 && paneIdOf(0) === sourcePaneId,
      'Closing the Browser tab closed its pane or ended its terminal');
    // ---- T3 section 9: a terminal selection into a thread's draft ----------
    const firstPane = document.querySelector<HTMLElement>('.terminal-pane')!;
    const shownScreen = [...firstPane.querySelectorAll<HTMLElement>('.xterm-screen')]
      .find(screen => screen.getBoundingClientRect().width > 0);
    assert(shownScreen, 'The first pane shows no terminal');
    const shownId = shownScreen.closest<HTMLElement>('[data-terminal-id]')!.dataset.terminalId!.split(':')[0]!;
    emit(shownId, '\x1b[2J\x1b[3J\x1bc\x1b[HFAIL src/a.test.ts\r\n  expected 1, received 2\r\n');
    await until(() => outstanding === 0, 'The selection marker did not drain');
    await frames(2);
    const screenBox = shownScreen.getBoundingClientRect();
    const corner = { clientX: screenBox.left + 1, clientY: screenBox.top + 1 };
    const far = { clientX: screenBox.right - 2, clientY: screenBox.bottom - 2 };
    shownScreen.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0, buttons: 1, detail: 1, ...corner }));
    shownScreen.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, buttons: 1, ...far }));
    shownScreen.dispatchEvent(new MouseEvent('mouseup', { bubbles: true, button: 0, detail: 1, ...far }));
    await wait(60);
    const more = firstPane.querySelector<HTMLButtonElement>('button[aria-label="More pane actions"]');
    assert(more, 'The pane has no ··· menu');
    flushSync(() => more.click());
    await until(() => [...document.querySelectorAll('[role="menuitem"]')]
      .some(item => item.textContent === 'Send selection to thread…'), 'The ··· menu offers no Send selection to thread…');
    const sendItem = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
      .find(item => item.textContent === 'Send selection to thread…')!;
    flushSync(() => sendItem.click());
    await until(() => document.querySelector('.composer-ref-choice') !== null, 'The thread picker did not open');
    assert(document.querySelector('.composer-ref-excerpt')?.textContent?.includes('expected 1, received 2') === true,
      'The picker does not preview the selection');
    flushSync(() => document.querySelector<HTMLButtonElement>('.composer-ref-choice')!.click());
    await until(() => sentSelections.length === 1, 'Choosing the thread added nothing');
    const sent = sentSelections[0]!;
    assert(sent.key === 'claude#review' && sent.title === terminals.find(t => t.id === shownId)!.title,
      `The selection went to ${sent.key} as ${sent.title}`);
    assert(sent.selection.trimEnd() === 'FAIL src/a.test.ts\n  expected 1, received 2',
      `The terminal handed over ${JSON.stringify(sent.selection.trimEnd())}`);
    const sentDraft = threadDrafts.get('claude#review');
    assert(sentDraft === `From the terminal "${sent.title}":\n\`\`\`\nFAIL src/a.test.ts\n  expected 1, received 2\n\`\`\`\n\n`,
      `The thread's draft reads ${JSON.stringify(sentDraft)}`);
    assert(document.querySelector('.composer-ref-choice') === null, 'The picker stayed open');
    assert(errors.length === 0, errors.join('; '));
    return { workspaceSleepRouting: 'passed', terminalSelectionToThread: 'drafted', resumeReplay: 'passed', panes: 4, historyLinesPerPane: 5200, maxAnnouncementLength, maxAnimationFrameMs,
      cursorRedrawsPerPane: 80 * 256, repeatedFocusCommits, backgroundTitleFits,
      renderFanOut: { backgroundTitleViewRenders, backgroundTitlePaneRenders, equalEchoCommits,
        activityViewRenders, activityPaneRenders, focusMovePaneRenders, focusMoveViewRenders,
        splitterFrameCommits, splitterViewRenders, splitterPaneRenders, heldSplitterCommits,
        backgroundTitleBrowserRenders },
      browserPane: 'tab in its pane, anchored, kept through tidy, closed',
      keystrokes: inputLatency.length, switches: switchLatency.length, scrollback: 'moved',
      maxInputDispatchMs: Math.max(...inputLatency), maxSwitchFocusMs: Math.max(...switchLatency),
      outstanding, remounts: 0 };
  } finally {
    if (output !== undefined) clearInterval(output);
    flushSync(() => root.render(null));
    FitAddon.prototype.fit = fit;
    viewType.render = renderView;
    paneType.type = renderPane;
    browserType.type = renderBrowser;
  }
}
