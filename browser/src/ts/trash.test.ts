// browser/src/ts/trash.test.ts
import { describe, it, expect } from 'vitest';
import { inRoot, fsOf, canTrash, trashAfter, type TrashViewer } from './trash';

const FSS = ['/red/nas1', '/red/nas2', '/red/nas2/deep'];
const viewer = (v: Partial<TrashViewer>): TrashViewer => ({
  username: 'alice', isAdmin: false, roots: [], filesystems: FSS, ...v,
});

describe('inRoot', () => {
  it('matches the root and below on a / boundary', () => {
    expect(inRoot('/red/nas1', '/red/nas1')).toBe(true);
    expect(inRoot('/red/nas1/', '/red/nas1/a')).toBe(true);
    expect(inRoot('/red/nas1', '/red/nas10')).toBe(false);
    expect(inRoot('/', '/anything')).toBe(true);
  });
});

describe('fsOf', () => {
  it('picks the longest listed prefix', () => {
    expect(fsOf(FSS, '/red/nas2/deep/x')).toBe('/red/nas2/deep');
    expect(fsOf(FSS, '/red/nas2/x')).toBe('/red/nas2');
    expect(fsOf(FSS, '/red/nas10/x')).toBeNull();
    expect(fsOf([], '/red/nas1/x')).toBeNull();
  });
});

describe('canTrash', () => {
  it('lets a regular user trash only their own items', () => {
    expect(canTrash(viewer({}), '/red/nas1/a', 'alice')).toBe(true);
    expect(canTrash(viewer({}), '/red/nas1/a', 'bob')).toBe(false);
    expect(canTrash(viewer({}), '/red/nas1/a', null)).toBe(false);
  });

  it('lets admins trash anything and storage admins inside their roots', () => {
    expect(canTrash(viewer({ isAdmin: true }), '/red/nas1/a', 'bob')).toBe(true);
    const sa = viewer({ username: 'sa', roots: ['/red/nas1'] });
    expect(canTrash(sa, '/red/nas1/a', 'bob')).toBe(true);
    expect(canTrash(sa, '/red/nas2/a', 'bob')).toBe(false);
    expect(canTrash(sa, '/red/nas2/a', 'sa')).toBe(true);
  });

  it('hides the icon for UNK owners (UIDs with no account)', () => {
    expect(canTrash(viewer({ isAdmin: true }), '/red/nas1/old', 'UNK')).toBe(false);
  });

  it('hides the icon on filesystem rows, .trash, .stats and outside FILESYSTEMS', () => {
    const admin = viewer({ isAdmin: true });
    for (const p of ['/red/nas1', '/red/nas2/deep', '/red/nas1/.trash/x', '/red/nas1/.archive/x.tar.gz', '/red/nas1/.stats', '/opt/x']) {
      expect(canTrash(admin, p, 'bob'), p).toBe(false);
    }
    expect(canTrash(viewer({ isAdmin: true, filesystems: [] }), '/red/nas1/a', 'bob')).toBe(false);
  });
});

describe('trashAfter', () => {
  it('is 24h after queued_at, in ms', () => {
    expect(trashAfter(1_000)).toBe((1_000 + 86_400) * 1000);
  });

  it('uses the server undo window when given', () => {
    expect(trashAfter(1_000, 0)).toBe(1_000_000);
    expect(trashAfter(1_000, 1)).toBe((1_000 + 3_600) * 1000);
  });
});
