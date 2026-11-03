// browser/src/ts/cleanupQueue.test.ts
// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { TrashEntry } from './models';

// In-memory stand-in for the server's queue.json and history.json.
const server: TrashEntry[] = [];
const history: any[] = [];
const entry = (path: string, owner: string): TrashEntry =>
  ({ path, size: 10, owner, actor: owner, fs: '/red/nas1', queued_at: 1_000 });

vi.mock('$api', () => ({
  api: {
    trashList: vi.fn(async () => server.slice()),
    trashAdd: vi.fn(async (path: string, size: number, owner: string) => {
      const e = { ...entry(path, owner), size };
      // Like the server: a new folder absorbs its queued children.
      for (let i = server.length - 1; i >= 0; i--) {
        if (server[i].path.startsWith(path + '/')) server.splice(i, 1);
      }
      server.push(e);
      return e;
    }),
    trashHistory: vi.fn(async () => history.slice()),
    trashKeep: vi.fn(async (path: string) => {
      const i = server.findIndex((e) => e.path === path);
      if (i < 0) throw new Error('not in the trash queue');
      return server.splice(i, 1)[0];
    }),
    cleanupScript: vi.fn(async () => new Blob(['x'])),
    cleanupNotify: vi.fn(async () => ({ sent: true, to: 'bob@x' })),
  },
}));

const q = await import('./cleanupQueue.svelte');

beforeEach(async () => {
  server.length = 0;
  history.length = 0;
  localStorage.clear();
  await q.loadCleanupQueue();
});

describe('cleanupQueue (server-backed)', () => {
  it('loads the cart from the server, not localStorage', async () => {
    localStorage.setItem('cleanupQueue.v1', JSON.stringify([{ path: '/stale', owner: 'x', size: 1 }]));
    server.push(entry('/red/nas1/a', 'bob'));
    await q.loadCleanupQueue();
    expect(q.cleanupQueue.items.map((i) => i.path)).toEqual(['/red/nas1/a']);
    expect(q.cleanupQueue.items[0].queued_at).toBe(1_000);
  });

  it('adds through the server and keeps the returned entry', async () => {
    await q.addToCleanupQueue('/red/nas1/b', 99, 'bob');
    expect(server.map((e) => e.path)).toEqual(['/red/nas1/b']);
    expect(q.cleanupQueue.items.map((i) => [i.path, i.size, i.fs])).toEqual([['/red/nas1/b', 99, '/red/nas1']]);
    expect(localStorage.getItem('cleanupQueue.v1')).toBeNull();
  });

  it('Keep removes the item on the server', async () => {
    await q.addToCleanupQueue('/red/nas1/b', 1, 'bob');
    await q.removeFromCleanupQueue('/red/nas1/b');
    expect(server).toEqual([]);
    expect(q.cleanupQueue.items).toEqual([]);
  });

  it('a failed add leaves the cart unchanged and rethrows', async () => {
    const { api } = await import('$api');
    (api.trashAdd as any).mockRejectedValueOnce(new Error('forbidden'));
    await expect(q.addToCleanupQueue('/red/nas1/c', 1, 'bob')).rejects.toThrow('forbidden');
    expect(q.cleanupQueue.items).toEqual([]);
  });

  it('Script and Notify no longer empty the cart', async () => {
    await q.addToCleanupQueue('/red/nas1/b', 1, 'bob');
    URL.createObjectURL = vi.fn(() => 'blob:x');
    URL.revokeObjectURL = vi.fn();
    await q.downloadScript('bob');
    await q.notifyUser('bob');
    expect(q.cleanupQueue.items.map((i) => i.path)).toEqual(['/red/nas1/b']);
    expect(server.length).toBe(1);
  });
});

describe('cleanupQueue extras', () => {
  it('adding a folder drops its queued children from the cart', async () => {
    await q.addToCleanupQueue('/red/nas1/a/b', 1, 'bob');
    await q.addToCleanupQueue('/red/nas1/a', 2, 'bob');
    expect(q.cleanupQueue.items.map((i) => i.path)).toEqual(['/red/nas1/a']);
  });

  it('toggleTrash queues, then keeps', async () => {
    await q.toggleTrash('/red/nas1/t', 1, 'bob');
    expect(q.isQueued('/red/nas1/t')).toBe(true);
    await q.toggleTrash('/red/nas1/t', 1, 'bob');
    expect(q.isQueued('/red/nas1/t')).toBe(false);
    expect(server).toEqual([]);
  });

  it('loads what the nightly run did', async () => {
    history.push({ ...entry('/red/nas1/old', 'bob'), status: 'moved', detail: '', dest: '/red/nas1/.trash/x', done_at: 5 });
    await q.loadTrashHistory();
    expect(q.cleanupQueue.history.map((h) => [h.path, h.status])).toEqual([['/red/nas1/old', 'moved']]);
  });
});
