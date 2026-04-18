// browser/src/ts/scan.svelte.ts
//
// Shared scan state. The top-bar Scan button (ScanPanel) and the bottom-bar
// progress strip (StatusBar) both read from this store — that's how we keep
// the button separate from the progress indicator while they share a single
// source of truth.

type Stage = "scan" | "sum" | "index" | "";

export const scanStatus = $state({
  running: false,
  stage: "" as Stage,
  percent: 0,
  message: "",
  error: "",
  // Generic busy indicator for non-scan async ops (delete, etc.). Renders as
  // a spinner + label in the status bar; UI stays interactive meanwhile.
  busy: false,
  busyLabel: "",
});

export function resetScanStatus() {
  scanStatus.running = false;
  scanStatus.stage = "";
  scanStatus.percent = 0;
  scanStatus.message = "";
  scanStatus.error = "";
  scanStatus.busy = false;
  scanStatus.busyLabel = "";
}
