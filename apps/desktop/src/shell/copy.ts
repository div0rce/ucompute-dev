// Copy with a grid cell focused and no text selected copies the row's identifier (the row's `data-copy`: device id,
// model id, job id or event JSON). A document `copy` handler, so macOS Edit › Copy and ⌘C never both fire; with text
// selected the browser's own copy is untouched.

export function installCopyHandler(): () => void {
  const onCopy = (e: ClipboardEvent) => {
    const sel = document.getSelection();
    // Range.collapsed, not Selection.isCollapsed: the same in browsers, and jsdom only implements the former.
    if (sel && sel.rangeCount > 0 && !sel.getRangeAt(0).collapsed) return;
    const cell = (document.activeElement as HTMLElement | null)?.closest?.('[role="grid"] [role="gridcell"]');
    const text = cell?.closest("[data-copy]")?.getAttribute("data-copy");
    if (!text || !e.clipboardData) return;
    e.clipboardData.setData("text/plain", text);
    e.preventDefault();
  };
  document.addEventListener("copy", onCopy);
  return () => document.removeEventListener("copy", onCopy);
}
