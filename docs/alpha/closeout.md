# Current candidate closeout and owner decisions

The source alpha now covers the qualified implementation through Wave 4.
Local consolidation, verification, integration and recovery are recorded in
[the consolidation status](../consolidation.md). Cargo stays at the existing
unreleased `0.1.0-alpha.1`; `publish = false` remains set. Historical October 19
closeout text is retained at Git revision `0f1a1b4`.

## Approved source publication

The owner approved Apache-2.0 on 26 October 2026. The standard root LICENSE,
updated NOTICE and Cargo license metadata are installed; original third-party
licenses and source-change records remain intact. The source repository is
[chronozarr/tile-ripper](https://github.com/chronozarr/tile-ripper), and
[tileripper.com](https://tileripper.com) hosts its documentation.

The old `jameshgrn/tile-ripper` URL redirects to `chronozarr/chronozarr` following
the earlier project migration. The new acquisition repository preserves that
redirect and keeps the tools separate under the same organization. No ChronoZarr
history is overwritten.

## Remaining human trial

Choose the first scientist/tester and agreed feedback channel. The
[updated trial](human-trial.md) is ready; automated installation and data checks
do not establish human usability. No invitation has been sent.

GitHub preserves the tracked source and history. Ignored scientific captures and
private recovery files remain local and are not included in the public repository.
The existing notice exceptions remain explicit for any later binary redistribution.

## Source handoff

Freeze the selected source commit and use [the source-handoff procedure](../consolidation/source-handoff.md).
Include the tracked vendored crates, their licenses and patches; the older
non-vendored procedure is superseded. Verify the source archive checksum and run
the isolated installation/live-smoke procedure from that clean source. Keep the
actual source revision with trial feedback.

The automated smoke reopens native ERA5 and GOES output, but does not establish
usability for another person or qualify every scientific workflow. Broader private
access should follow resolution of actual trial failures. No package or binary release tag has been created.
