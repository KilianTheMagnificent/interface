import { expect, it } from 'vitest';
import fixture from '../fixtures/remote-phone-v4.json';
import inboxFixture from '../fixtures/remote-inbox-v2.json';
import type { TerminalAttentionV1 } from '../shared/ipc';
import type {
  RemoteThreadApprovalResponseV1, RemoteThreadDetailV1, RemoteThreadSendV1, RemoteThreadsV1,
  RemoteInboxV2, RemoteThreadStatus, RemoteWorkspaceSnapshotV4,
} from '../shared/remote';

// crates/interface-contracts (remote.rs, remote_threads.rs) round-trips these
// bytes and apps/interface-ios decodes them: one contract on all three sides.
const statuses: readonly RemoteThreadStatus[] = ['needsYou', 'working', 'idle', 'failed'];

it('reads the shared inbox without granting actions to completed or sessionless tasks', () => {
  const inbox = inboxFixture as RemoteInboxV2;
  expect(inbox.version).toBe(2);
  expect(inbox.items.map(item => item.source)).toEqual(['thread', 'agentTask', 'agentTask']);
  expect(inbox.items[1].thread).toBeUndefined();
  expect(inbox.items[1].canSend).toBe(false);
  expect(inbox.items[2].canSend).toBe(false);
  expect(inbox.items[2].detail).toContain('Completed');
});

it('reads roster v4 and phone Threads v1 as Rust writes them', () => {
  const roster = fixture.roster as RemoteWorkspaceSnapshotV4;
  expect(roster.version).toBe(4);
  const [waiting, quiet] = roster.workspaces[0]!.terminals;
  const attention: TerminalAttentionV1 | undefined = waiting!.attention;
  expect(attention).toEqual({ version: 1, since: '2026-10-14T09:12:00.000Z' });
  expect(quiet!.attention).toBeUndefined();
  expect(roster.projects[quiet!.id]).toBeNull();

  const threads = fixture.threads as RemoteThreadsV1;
  expect(threads.version).toBe(1);
  expect(threads.threads.map(thread => thread.status)).toEqual(statuses);
  for (const thread of threads.threads) {
    expect(thread.key).toBe(`${thread.engine}#${thread.sessionId}`);
    expect(Object.keys(thread).sort()).toEqual([
      'engine', 'key', 'lastActivity', 'section', 'sessionId', 'status', 'title', 'unread',
      ...(thread.project === undefined ? [] : ['project']),
    ].sort());
  }
  const detail = fixture.detail as RemoteThreadDetailV1;
  expect(detail.thread).toEqual(threads.threads[0]);
  expect(detail.approval?.answerable).toBe(true);
  const answer: RemoteThreadApprovalResponseV1 = { version: 1, approvalId: 'toolu_fixture', decision: 'allowOnce' };
  expect(fixture.approvalResponse).toEqual(answer);
  const followUp: RemoteThreadSendV1 = { version: 1, text: 'Run the migration tests again.' };
  expect(fixture.send).toEqual(followUp);
});
