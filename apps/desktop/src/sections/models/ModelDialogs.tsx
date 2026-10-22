import { useId, useState } from "react";
import { api, type Model } from "../../api/client";
import { explain, type MappedError } from "../../api/errors";
import { openExternal } from "../../daemon";
import { humanBytes } from "../../format";
import { Dialog } from "../../ui/Dialog";
import { ErrorNotice } from "../../ui/ErrorNotice";

/**
 * The download confirmation: licence, size, source and redistribution before anything is fetched (same content and
 * order as the old row confirmation). Confirm has the initial focus.
 */
export function DownloadDialog({ model, onClose, onConfirm }: { model: Model | null; onClose: () => void; onConfirm: (m: Model) => void }) {
  const m = model;
  return (
    <Dialog open={!!m} label={m ? `Confirm download of ${m.name}` : "Confirm download"} onClose={onClose}>
      {m && (
        <>
          <h2>Download {m.name}</h2>
          <p className="prose">
            <strong>License: {m.license || "unknown"}.</strong> {humanBytes(m.bytes)} will be downloaded from {m.source}.
            {m.redistribution && m.redistribution !== "unknown" ? ` Redistribution: ${m.redistribution}.` : ""}
          </p>
          <p className="prose muted small">Downloads can't be cancelled; a stopped download resumes next time.</p>
          <div className="dialog-actions">
            {m.license_url && (
              <button type="button" className="btn" onClick={() => void openExternal(m.license_url!)}>
                View license
              </button>
            )}
            <button type="button" className="btn" onClick={onClose}>
              Close
            </button>
            <button type="button" className="btn btn-primary" data-autofocus="" onClick={() => onConfirm(m)}>
              Download {humanBytes(m.bytes)}
            </button>
          </div>
        </>
      )}
    </Dialog>
  );
}

/** The daemon requires an absolute path (admin.rs `import`); POSIX, drive-letter or UNC. */
export const isAbsolutePath = (p: string) => p.startsWith("/") || /^[A-Za-z]:[\\/]/.test(p) || p.startsWith("\\\\");
/** Model ids the store accepts (`safe_name`). */
const ID_RE = /^[A-Za-z0-9._-]+$/;

/**
 * Import a local GGUF by absolute path (no native file picker: that needs plugin-dialog, plan §6). The daemon hashes
 * and links the file, refusing one without a valid GGUF header.
 */
export function ImportDialog({ open, onClose, onImported }: { open: boolean; onClose: () => void; onImported: (id: string) => void }) {
  const [path, setPath] = useState("");
  const [id, setId] = useState("");
  const [license, setLicense] = useState("");
  const [invalid, setInvalid] = useState<{ field: "path" | "id"; text: string } | null>(null);
  const [error, setError] = useState<MappedError | null>(null);
  const [busy, setBusy] = useState(false);
  const pathErr = useId();
  const idErr = useId();

  const close = () => {
    setInvalid(null);
    setError(null);
    onClose();
  };

  async function submit() {
    if (busy) return;
    const p = path.trim();
    const i = id.trim();
    if (!isAbsolutePath(p)) return setInvalid({ field: "path", text: "Enter the full path to the file, for example /Users/you/models/model.gguf or C:\\models\\model.gguf." });
    if (i && !ID_RE.test(i)) return setInvalid({ field: "id", text: "A model id may only contain letters, digits, '-', '_' and '.'." });
    setInvalid(null);
    setError(null);
    setBusy(true);
    try {
      const r = await api.importModel(p, { ...(i ? { id: i } : {}), ...(license.trim() ? { license: license.trim() } : {}) });
      setPath("");
      setId("");
      setLicense("");
      onImported(typeof r.id === "string" ? r.id : i);
      close();
    } catch (e) {
      setError(explain(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} label="Import a GGUF file" onClose={close}>
      <form
        className="import-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <h2>Import a GGUF file</h2>
        <p className="prose small muted">The file stays where it is; this computer hashes it and adds it to its models. Nothing is uploaded.</p>
        <label>
          <span>File path</span>
          <input
            data-autofocus=""
            value={path}
            onChange={(e) => setPath(e.target.value)}
            aria-invalid={invalid?.field === "path" || undefined}
            aria-describedby={invalid?.field === "path" ? pathErr : undefined}
            spellCheck={false}
            required
          />
        </label>
        {invalid?.field === "path" && (
          <p id={pathErr} className="field-error tone-danger">
            {invalid.text}
          </p>
        )}
        <label>
          <span>Model id (optional)</span>
          <input value={id} onChange={(e) => setId(e.target.value)} placeholder="From the file name" aria-invalid={invalid?.field === "id" || undefined} aria-describedby={invalid?.field === "id" ? idErr : undefined} spellCheck={false} />
        </label>
        {invalid?.field === "id" && (
          <p id={idErr} className="field-error tone-danger">
            {invalid.text}
          </p>
        )}
        <label>
          <span>License (optional)</span>
          <input value={license} onChange={(e) => setLicense(e.target.value)} />
        </label>
        {error && <ErrorNotice error={error} />}
        <div className="dialog-actions">
          <button type="button" className="btn" onClick={close}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" aria-disabled={busy || undefined}>
            {busy ? "Importing…" : "Import"}
          </button>
        </div>
      </form>
    </Dialog>
  );
}
