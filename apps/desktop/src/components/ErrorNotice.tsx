import type { MappedError } from "../api/errors";

export function ErrorNotice({ error, onDismiss }: { error: MappedError; onDismiss?: () => void }) {
  return (
    <div className="notice error" role="alert">
      <div className="title">{error.title}</div>
      <div>{error.next_step}</div>
      <div className="detail">
        {error.code}: {error.detail}
        {error.hint ? ` (${error.hint})` : ""}
      </div>
      {onDismiss && (
        <div className="actions">
          <button type="button" className="btn btn-sm" onClick={onDismiss}>
            Dismiss
          </button>
        </div>
      )}
    </div>
  );
}
