# Current candidate closeout and owner decisions

The private candidate now covers the qualified implementation through Wave 4.
Local consolidation, verification, integration and recovery are recorded in
[the consolidation status](../consolidation.md). Cargo stays at the existing
unreleased `0.1.0-alpha.1`; `publish = false` remains set. Historical October 19
closeout text is retained at Git revision `0f1a1b4`.

## Decisions needed from the owner

1. Confirm the owner(s), authority to license the authored source and historical
   study, and any employer/university/funder/collaborator review. Confirm whether
   any external code/text was copied or adapted beyond the recorded implementation
   and dependency forks. Approve Apache-2.0 or specify the intended license and
   scope/exclusions. Supply an exact copyright statement if desired. The proposed
   files remain [inactive review material](proposed-licensing/README.md).
2. Choose the off-machine private backup/remote destination and its access scope.
   Local recovery cannot protect against loss of this computer. No remote or
   public repository has been invented.
3. Choose the first scientist/tester and the agreed feedback channel after source
   access is approved. The [updated trial](human-trial.md) is ready; only a real
   tester can supply the human usability result. No invitation has been sent.

Local fork maintenance and a complete native candidate build have been selected
for this unreleased candidate. No upstream contact, publication, tag, upload or
binary distribution is required to finish local consolidation.

## Source handoff after approval

Freeze the approved commit and use [the source-handoff procedure](../consolidation/source-handoff.md).
Include the tracked vendored crates, their licenses and patches; the older
non-vendored procedure is superseded. Verify the source archive checksum and run
the isolated installation/live-smoke procedure from that clean source. Keep the
actual source revision with trial feedback.

The automated smoke reopens native ERA5 and GOES output, but does not establish
usability for another person or qualify every scientific workflow. Broader private
access should follow resolution of actual trial failures. Tagging/publication is
a separate owner action after the candidate is approved; no release tag exists.
