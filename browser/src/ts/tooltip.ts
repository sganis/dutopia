// browser/src/ts/tooltip.ts

export type Tip = {
  show: boolean;
  x: number;
  y: number;
  username?: string;
  value?: string;
  percent?: number;
};

const TOOLTIP_WIDTH = 240;
const TOOLTIP_HEIGHT = 110;
const MARGIN = 10;
const ARROW_GAP = 12;
const HIDE_DELAY = 1200;

export function createTooltipHandlers<TUser>(
  getTip: () => Tip,
  setTip: (next: Tip) => void,
  getValue: (user: TUser) => string
) {
  let hideTimer: number | null = null;

  function clampToViewport(rawX: number, rawY: number) {
    const ww = window.innerWidth;
    const wh = window.innerHeight;
    const halfW = TOOLTIP_WIDTH / 2;
    const minX = MARGIN + halfW;
    const maxX = ww - MARGIN - halfW;
    const minY = MARGIN + TOOLTIP_HEIGHT + ARROW_GAP;
    const maxY = wh - MARGIN;
    return {
      x: Math.min(maxX, Math.max(minX, rawX)),
      y: Math.min(maxY, Math.max(minY, rawY)),
    };
  }

  function scheduleHide(ms = HIDE_DELAY) {
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = window.setTimeout(() => {
      setTip({ show: false, x: 0, y: 0 });
      hideTimer = null;
    }, ms);
  }

  function cancelHide() {
    if (hideTimer) {
      clearTimeout(hideTimer);
      hideTimer = null;
    }
  }

  function showTip(e: MouseEvent, userData: TUser, percent: number) {
    cancelHide();
    const { x, y } = clampToViewport(e.clientX, e.clientY);
    setTip({
      show: true,
      x,
      y,
      username: (userData as any).username,
      value: getValue(userData),
      percent: Math.round(percent * 10) / 10,
    });
  }

  function moveTip(e: MouseEvent) {
    const current = getTip();
    if (!current.show) return;
    cancelHide();
    const { x, y } = clampToViewport(e.clientX, e.clientY);
    setTip({ ...current, x, y });
  }

  function hideTip() {
    cancelHide();
    setTip({ show: false, x: 0, y: 0 });
  }

  return { showTip, moveTip, hideTip, scheduleHide, cancelHide };
}
