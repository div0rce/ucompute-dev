# Security reporting and escalation

Adopted 2026-11-06 (D-036). The route is **not verified end to end yet**, and the external-alpha security gate stays open until
the table below is complete. Public text names the role, never a person.

## Route

Reports go to `security@ucompute.org`, or a private GitHub security advisory once the source is public. The **interim
responsible responder** holds the mailbox and owns every report until it is closed. A **backup responder** is required
before the gate closes and is not named yet.

## Handling a report

1. **Acknowledge** within 7 days, from the same address (volunteer target, not an SLA). Record the date received and a case id
   in a private location, never in a public issue, and keep only redacted evidence.
2. **Reproduce and classify** on the current release and on `main`. Critical or high: a default-configuration path to remote code
   execution, prompt or key disclosure, a policy or local-only bypass, or data loss. Medium or low: anything needing a privileged
   position or an owner-enabled setting.
3. **Escalate.** The responder hands a confirmed report to Platform Engineering, which owns the fix (D-034). Critical or high
   stops promotion of any release in the affected scope until it is fixed or scoped out. An upstream defect is reported upstream as well, and
   its disposition is recorded as in `THREAT_MODEL.md`, "RPC CVE disposition".
4. **Fix and release** through the normal promotion record. Distribution stays the owner's decision. Rehearse revocation
   and rollback for anything already shipped.
5. **Update** the reporter at least every 14 days, and **disclose** after a fix, or at 90 days unless agreed otherwise, with credit
   if the reporter wants it.

If the responder is unavailable for 7 days the backup takes over; if neither can respond, no external build is promoted until
one can.

## Verification of the route

| Check | State | Evidence |
|---|---|---|
| The address exists | **Done 2026-11-06.** `route2.mx.cloudflare.net` answered `250` to `RCPT TO:<security@ucompute.org>` and `550 5.1.1 Address does not exist` to a made-up address on the same domain. No message was sent | Engineering, SMTP session stopped before `DATA` |
| Domain mail records | MX on Cloudflare routing, SPF set; **no DMARC record** (optional hardening, owner's DNS) | `dig`, 2026-11-06 |
| Delivery: a harmless test from an outside address reaches the responder's mailbox | **Open** | Date, sender domain, time received |
| Reply: the responder answers from the route and the outside address receives it | **Open** | Date, time received |
| Response ownership: responder confirmed, backup named | **Open**: responder approved by the owner (D-036); backup not named | `docs/DECISIONS.md` |

The test report goes in `docs/evidence/security-route-test-<date>.md`, with mail headers reduced to dates and domains.
