import { useEffect, useState } from "react";
import { Dialog } from "../ui/Dialog";
import { isMac } from "../ui/contextMenu";
import { modLabel, SHORTCUTS_EVENT } from "./shortcuts";

/** Mod+/ : the keyboard map (plan §1.3) in a dialog. */
export function ShortcutsSheet() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const on = () => setOpen(true);
    window.addEventListener(SHORTCUTS_EVENT, on);
    return () => window.removeEventListener(SHORTCUTS_EVENT, on);
  }, []);
  const m = modLabel();
  const rows: [string, string][] = [
    [`${m}1 … ${m}7`, "Go to Dashboard, Devices, Models, Jobs, Chat, Activity, Settings"],
    [`${m},`, "Settings"],
    [`${m}F`, "Focus the filter box"],
    ["Esc", "Close a menu or dialog, cancel a confirmation, clear the filter; in Chat, stop generating"],
    ["F6 / Shift+F6", "Next / previous region: sections, toolbar, scope, table, inspector, status bar"],
    ["↑ ↓ Home End PgUp PgDn", "Move the selection in a table"],
    ["← →", "Move between cells of the selected row"],
    ["Enter", "Open the selected row's details"],
    ["Shift+F10", "Context menu for the selected row"],
    [`${m}C`, "Copy the selected row's identifier (when no text is selected)"],
    [isMac() ? "⌘⌫" : "Delete", "Remove the selected device or delete the selected model (asks first)"],
    [`${m}I`, "Show or hide the details panel"],
    [isMac() ? "⌘R" : "F5, Ctrl+R", "Refresh"],
    [`${m}/`, "This list"],
    ["Enter / Shift+Enter", "In Chat: send / new line"],
  ];
  return (
    <Dialog open={open} label="Keyboard shortcuts" onClose={() => setOpen(false)}>
      <h2>Keyboard shortcuts</h2>
      <table className="shortcuts">
        <tbody>
          {rows.map(([k, v]) => (
            <tr key={k}>
              <th scope="row">
                <kbd>{k}</kbd>
              </th>
              <td>{v}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="dialog-actions">
        <button type="button" className="btn" data-autofocus="" onClick={() => setOpen(false)}>
          Close
        </button>
      </div>
    </Dialog>
  );
}
