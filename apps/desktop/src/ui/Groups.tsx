import { useState } from "react";
import { api, shortLabel, type Group, type Peer } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { plural } from "../format";
import { groups as groupsStore, mesh } from "../state/live";
import { ErrorNotice } from "./ErrorNotice";

/** PUT one whole group; every field the editor does not show (the pooled flag) is sent back unchanged. */
async function put(group: Group, onChange: (g: Group[]) => void, onError: (e: unknown) => void, setSaving: (b: boolean) => void) {
  setSaving(true);
  try {
    onChange(await api.putGroup(group));
  } catch (e) {
    onError(e);
  } finally {
    setSaving(false);
  }
}

/**
 * Household groups (ADR-0003 D7): which paired devices may run inference for each other. The daemon's default is
 * deny, so a freshly paired device does nothing until a group policy allows it. Pooled inference does not exist, so
 * its flag is never shown (it is round-tripped on PUT). There is no delete route (N9).
 */
export function Groups({ groups, peers, onChange, onError }: { groups: Group[]; peers: Peer[]; onChange: (g: Group[]) => void; onError: (e: unknown) => void }) {
  const [newName, setNewName] = useState("");
  const [saving, setSaving] = useState(false);
  const nameOf = (id: string) => peers.find((p) => p.id === id)?.name || shortLabel(id);
  const save = (g: Group) => put(g, onChange, onError, setSaving);
  const update = (name: string, f: (g: Group) => Group) => {
    const g = groups.find((x) => x.name === name);
    if (g) void save(f(g));
  };

  return (
    <div className="section groups-editor">
      <h2>Groups</h2>
      <p className="muted small prose">
        A device can only run work for another device in the same group, and only what the group allows. Prompts are off for a new group. Remove all members to stop a
        group applying.
      </p>
      {groups.length === 0 && <div className="empty">No groups yet. Create one and add paired devices to it.</div>}
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
            <label className="check">
              <input type="checkbox" checked={g.policy.allow_remote_inference} disabled={saving} onChange={(e) => update(g.name, (x) => ({ ...x, policy: { ...x.policy, allow_remote_inference: e.target.checked } }))} />
              Members may run prompts for each other
            </label>
            <span id={`group-${g.name}-help`} className="visually-hidden">
              Group {g.name} with {plural(g.members.length, "device")}.
            </span>
          </fieldset>
        );
      })}
      <form
        className="group-new"
        onSubmit={(e) => {
          e.preventDefault();
          const name = newName.trim();
          if (!name || groups.some((g) => g.name === name)) return;
          setNewName("");
          void save({ name, members: [], policy: { allow_remote_inference: false, allow_pooled_inference: false } });
        }}
      >
        <label>
          <span className="small muted">New group</span>
          <input type="text" value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="Home" maxLength={32} />
        </label>
        <button type="submit" className="btn" disabled={saving || !newName.trim()}>
          Create group
        </button>
      </form>
    </div>
  );
}

const setGroups = (g: Group[]) => groupsStore.set({ data: g, at: Date.now(), error: null });

/** The Groups editor over the live stores (Settings › Sharing & permissions). */
export function GroupsEditor() {
  const g = groupsStore.use();
  const m = mesh.use();
  const [error, setError] = useState<MappedError | null>(null);
  return (
    <>
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}
      <Groups groups={g.data ?? []} peers={m.data?.peers ?? []} onChange={(x) => (setGroups(x), setError(null))} onError={(e) => setError(explain(e))} />
    </>
  );
}

/** One device's groups (Devices › Permissions): membership and each group's prompt switch. */
export function GroupMembership({ peerId }: { peerId: string }) {
  const g = groupsStore.use().data ?? [];
  const name = mesh.use().data?.peers.find((p) => p.id === peerId)?.name || shortLabel(peerId);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<MappedError | null>(null);
  const save = (group: Group) => put(group, (x) => (setGroups(x), setError(null)), (e) => setError(explain(e)), setSaving);
  if (g.length === 0) return <p className="muted small">No groups yet. Create one under Settings › Sharing &amp; permissions.</p>;
  return (
    <fieldset className="group-membership">
      <legend>Groups {name} is in</legend>
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}
      {g.map((x) => {
        const member = x.members.includes(peerId);
        return (
          <div key={x.name} className="membership-row">
            <label className="check">
              <input
                type="checkbox"
                checked={member}
                disabled={saving}
                onChange={() => void save({ ...x, members: member ? x.members.filter((m) => m !== peerId) : [...x.members, peerId] })}
              />
              In {x.name}
            </label>
            {member && (
              <label className="check">
                <input type="checkbox" checked={x.policy.allow_remote_inference} disabled={saving} onChange={(e) => void save({ ...x, policy: { ...x.policy, allow_remote_inference: e.target.checked } })} />
                Members of {x.name} may run prompts for each other
              </label>
            )}
          </div>
        );
      })}
    </fieldset>
  );
}
