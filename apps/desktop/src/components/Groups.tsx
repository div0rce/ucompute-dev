import { useState } from "react";
import { api, type Group, type Peer } from "../api/client";

/**
 * Household groups (ADR-0003 D7): which paired devices may run inference for each other. The daemon's
 * default is deny, so a freshly paired device does nothing until a group policy allows it.
 */
export function Groups({ groups, peers, onChange, onError }: { groups: Group[]; peers: Peer[]; onChange: (g: Group[]) => void; onError: (e: unknown) => void }) {
  const [newName, setNewName] = useState("");
  const [saving, setSaving] = useState(false);
  const nameOf = (id: string) => peers.find((p) => p.id === id)?.name || `${id.slice(0, 4)}-${id.slice(4, 8)}-${id.slice(8, 12)}`;

  async function save(next: Group[]) {
    setSaving(true);
    try {
      onChange(await api.setGroups(next));
    } catch (e) {
      onError(e);
    } finally {
      setSaving(false);
    }
  }
  const update = (name: string, f: (g: Group) => Group) => save(groups.map((g) => (g.name === name ? f(g) : g)));

  return (
    <div className="section">
      <h2>Groups</h2>
      <p className="muted small" style={{ maxWidth: "64ch" }}>
        A device can only run work for another device in the same group, and only what the group allows. Both are off for a new group.
      </p>
      {groups.length === 0 && <div className="empty" style={{ marginTop: 10 }}>No groups yet. Create one and add paired devices to it.</div>}
      {groups.map((g) => {
        const candidates = peers.filter((p) => !g.members.includes(p.id));
        return (
          <fieldset key={g.name} className="group" aria-describedby={`group-${g.name}-help`}>
            <legend>
              <strong>{g.name}</strong>
            </legend>
            <div className="group-members">
              {g.members.length === 0 && <span className="muted small">No devices in this group.</span>}
              {g.members.map((id) => (
                <span key={id} className="tag member">
                  {nameOf(id)}
                  <button type="button" className="tag-x" aria-label={`Remove ${nameOf(id)} from ${g.name}`} disabled={saving} onClick={() => update(g.name, (x) => ({ ...x, members: x.members.filter((m) => m !== id) }))}>
                    ×
                  </button>
                </span>
              ))}
              {candidates.length > 0 && (
                <label className="small">
                  <span className="visually-hidden">Add a device to {g.name}</span>
                  <select value="" disabled={saving} onChange={(e) => e.target.value && update(g.name, (x) => ({ ...x, members: [...x.members, e.target.value] }))}>
                    <option value="">Add a device</option>
                    {candidates.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name || p.short_label}
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
            <div className="radio-row" style={{ marginTop: 8 }}>
              <label className="check">
                <input type="checkbox" checked={g.policy.allow_remote_inference} disabled={saving} onChange={(e) => update(g.name, (x) => ({ ...x, policy: { ...x.policy, allow_remote_inference: e.target.checked } }))} />
                Members may run prompts for each other
              </label>
              <label className="check">
                <input type="checkbox" checked={g.policy.allow_pooled_inference} disabled={saving} onChange={(e) => update(g.name, (x) => ({ ...x, policy: { ...x.policy, allow_pooled_inference: e.target.checked } }))} />
                Members may pool memory for one model
              </label>
            </div>
            <span id={`group-${g.name}-help`} className="visually-hidden">
              Group {g.name} with {g.members.length} devices.
            </span>
          </fieldset>
        );
      })}
      <form
        className="toolbar"
        onSubmit={(e) => {
          e.preventDefault();
          const name = newName.trim();
          if (!name || groups.some((g) => g.name === name)) return;
          setNewName("");
          save([...groups, { name, members: [], policy: { allow_remote_inference: false, allow_pooled_inference: false } }]);
        }}
      >
        <label>
          <span className="small muted">New group</span>
          <br />
          <input type="text" value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="Home" maxLength={32} />
        </label>
        <button type="submit" className="btn" disabled={saving || !newName.trim()} style={{ alignSelf: "end" }}>
          Create group
        </button>
      </form>
    </div>
  );
}
