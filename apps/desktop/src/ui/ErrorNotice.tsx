import type { MappedError } from "../api/errors";

export function ErrorNotice({ error, onDismiss }: { error: MappedError; onDismiss?: () => void }) {
  return (
    <div className="notice tone-danger" role="alert">
      <div className="notice-title">{error.title}</div>
      <div>{error.next_step}</div>
      <div className="notice-detail">
        {error.code}: {error.detail}
        {error.hint ? ` (${error.hint})` : ""}
      </div>
      {onDismiss && (
        <div className="notice-actions">
          <button type="button" className="btn btn-sm" onClick={onDismiss}>
            Dismiss
          </button>
        </div>
      )}
    </div>
  );
}
