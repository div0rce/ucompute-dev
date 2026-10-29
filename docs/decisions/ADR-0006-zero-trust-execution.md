# ADR-0006: Zero-trust distributed execution and security gates SG-1 to SG-3

- **Status:** Proposed (not Accepted). The owner pasted this architecture into the engineering session on 2026-10-29
  without asking for it to be adopted; it becomes Accepted when the owner says so.
- **Date:** 2026-10-29
- **Inputs:** ADR-0003 (peer transport), ADR-0005 (split inference) and its D4 stage filter; the Core Lab's
  `integration/RPC-SURFACE.md` (ucompute-core-lab 78a84f2): llama.cpp's rpc-server must be treated as unsafe against a
  hostile head, because a byte filter cannot establish its memory safety; WP-04 plan OD-8 (sandbox for rpc-server);
  `docs/security/THREAT_MODEL.md`; spec section 11 gates.
- **Implementation:** nothing in this ADR is built. Today's code operates inside SG-1 only (owner-controlled devices).

## 1. Context

The stage-side filter (ADR-0005 D4) closes the known capability paths in llama.cpp's RPC protocol, but it is a safety
boundary for the current phase, not the architecture UCompute needs. UCompute's aim includes computers run by people
the requester does not know. Two problems follow, and encryption on the wire solves neither:

| Threat | Required protection |
|---|---|
| A malicious requester attacks the computer that runs its work | Isolated workers, least privilege, authenticated requests, strict quotas, a trusted local broker |
| A malicious provider reads prompts, models or intermediate activations, or fabricates results | Trust-aware placement, confidential execution where the hardware supports it, data-sensitivity policies, result verification |

No architecture removes the need for ongoing vulnerability management; the aim is a system that stays defensible
as it changes, and that contains a future flaw instead of exposing every participant.

## 2. Decision (proposed)

UCompute will never rely on peer trust alone for the security of distributed execution. Its architecture requires:

1. **A trusted local broker.** Remote peers never drive an inference engine directly. They submit constrained, typed
   execution requests; the broker decides what runs. No arbitrary commands, file paths, model downloads, process
   launches or configuration changes from a peer.
2. **Isolated workers.** A compromised engine gains no access to the owner's files, credentials, other processes or
   network. Isolation is enforced by the operating system: Landlock and seccomp (plus further mechanisms) on Linux,
   App Sandbox on macOS, AppContainer on Windows; virtual machines where practical, with GPU support and performance
   validated per platform.
3. **Bounded resources.** Peers cannot monopolize CPU, GPU, memory, storage or bandwidth; owners control participation
   and can revoke it.
4. **Continuous verification.** Threat modelling, fuzzing, hostile-peer tests, dependency scanning, independent
   review, signed releases and vulnerability response are permanent engineering obligations.
5. **Fail closed.** Untrusted participation is refused wherever these protections cannot be enforced. Confidential
   workloads are not placed on providers without a validated confidentiality mechanism. No platform or hardware
   backend is exempted to reach feature parity or performance.

Private Mesh (trusted, owner-controlled) and public Swarm (untrusted) computation are distinguished at the protocol,
scheduling and security-policy levels.

## 3. Security gates

| Gate | Requirement | Allows |
|---|---|---|
| **SG-1, Trusted Mesh** | Authenticated peers; constrained stage operations (the ADR-0005 D4 filter); strict authorization (default deny, pooled policy on both ends); resource limits; a validated private-device threat model | Controlled private alpha among the owner's own devices: today's WP-04 scope and P1 |
| **SG-2, Isolated distributed execution** | Platform-specific worker isolation; hostile-request tests; an escape-resistance assessment; independent review | Broader use, platform by platform, on platforms that pass |
| **SG-3, Untrusted Swarm** | A provider confidentiality policy; result-integrity defences; abuse prevention; resource accounting; security response; adversarial testing | Public contribution |

Passing SG-2 does not authorize SG-3. Each gate is evidence-based like the spec's gates: a platform is inside a gate
only with recorded evidence for it. (Named SG-n to avoid a clash with WP-04's "S1" single-host runs.)

## 4. Consequences

- WP-04 stays inside SG-1: split sessions only between devices the owner controls, the D4 filter enforced on the
  stage host, and the R-side per-session byte cap (lab review of C5) required before anything beyond SG-1.
- OD-8 (a sandbox for rpc-server) becomes part of SG-2 instead of an optional hardening item.
- Engineering and the Core Lab design the execution broker and the isolation interface so that today's stage server
  can later run behind them without a rewrite; this is design work, not scheduled ahead of P1.
- The spec's Gate F (public swarm) is read together with SG-3.

## 5. Open questions

- Which isolation mechanism per platform keeps GPU access (Metal, Vulkan, CUDA) usable; measured per platform.
- What result verification is practical for inference (redundant execution, spot checks, attested hardware).
- Where confidential computing (confidential VMs, GPU attestation) applies, given that it needs specific hardware.
