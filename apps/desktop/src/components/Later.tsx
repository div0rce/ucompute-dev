/** Honest placeholder for views whose milestone has not arrived. No fake data. */
export function Later({ title, milestone, what }: { title: string; milestone: string; what: string }) {
  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">{title}</h1>
      </div>
      <div className="notice">
        <div className="title">Not available yet ({milestone})</div>
        <div>{what}</div>
      </div>
    </section>
  );
}
