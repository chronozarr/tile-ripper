# Source-bound WSE quality policies

The [PO.DAAC Cookbook raster recipe](https://podaac.github.io/tutorials/notebooks/datasets/SWOT_Raster_Notebook_local.html)
was found before implementation: it retains summary categories 0 and 1 using
`wse_qual < 2`. [cookbook.json](cookbook.json) records the recipe cells and source
notebook hash. This is a demonstrated selection example, not a universal rule
for acceptable river elevations. The PDD warns that suspect measurements can
have large errors, degraded measurements are very likely to, and bad values
should be ignored. Including suspect data is therefore an explicit choice.

The inspected Version D PGD0 file has summary codes **0 good, 1 suspect,
2 degraded, 3 bad**, with fill **255**. Preparation checks both the actual
`flag_values` and `flag_meanings`. The compiled bit table must match the file's
actual masks and names before a bitwise policy can run. Product attributes,
including their original bytes, remain in provenance. A schema match is not
qualification of every PGD0/PID0 scene or future release.

| Bit | Decimal mask | Source condition | Meaning in brief |
|---:|---:|---|---|
| 1 | 2 | classification_qual_suspect | Suspect input classification |
| 2 | 4 | geolocation_qual_suspect | Suspect input geolocation |
| 5 | 32 | large_uncert_suspect | Uncertainty above processing threshold |
| 7 | 128 | bright_land | Bright-land input contamination |
| 9 | 512 | specular_ringing_prior_water_suspect | Ringing intersecting prior water |
| 12 | 4096 | few_pixels | Few contributing PIXC samples |
| 13 | 8192 | far_range_suspect | Beyond useful far-range threshold |
| 14 | 16384 | near_range_suspect | Inside useful near-range threshold |
| 18 | 262144 | classification_qual_degraded | Degraded input classification |
| 19 | 524288 | geolocation_qual_degraded | Degraded input geolocation |
| 20 | 1048576 | dark_water_degraded | Dark-water inputs |
| 21 | 2097152 | low_coherence_water_degraded | Low-coherence water inputs |
| 22 | 4194304 | specular_ringing_prior_land_degraded | Ringing outside prior water |
| 24 | 16777216 | value_bad | Outside processing sanity limits |
| 26 | 67108864 | outside_data_window | Outside acquired data window |
| 28 | 268435456 | no_pixels | No assigned PIXC samples |
| 29 | 536870912 | outside_scene_bounds | Outside swath-aligned scene |
| 30 | 1073741824 | inner_swath | Inside inner-swath threshold |
| 31 | 2147483648 | missing_karin_data | Missing KaRIn data |

The table agrees with the actual D attributes and Table 15 of the official
[Raster PDD, February 24 2025, document Revision C](https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-docs/web-misc/swot_mission_docs/pdd/D-56416_SWOT_Product_Description_L2_HR_Raster_20250224a_RevC_clean_sig_final.pdf),
PDF/printed page 44. **Document Revision C is distinct from data release C.**
The inspected older PIC0 data file lacks bits 9 and 22. A PDD table alone would
have incorrectly implied those conditions were defined in that older file.
The Version D transition [documentation](https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-docs/web-misc/swot_mission_docs/SWOT_VersionD_KaRIn_Products_Release_Note_20250423b.pdf)
also describes the raster algorithm/flag changes. Physical compatibility here
is independently established by the actual PGD0 file, not inferred from the PDD.

The source's unsigned-word severity comment uses 0 = good, positive values
below 32768 = suspect, 32768 through 8388607 = degraded, and larger values = bad.
These ranges summarize severity; they do not assign meanings to unused bits.
The implementation does not reconstruct summary quality from those ranges.

`Policy::Raw` returns the source WSE nonmissing mask. HDF5 initialization alone
is not scientific missingness. CF markers and NaNs are handled independently;
raw infinities retain the inherited source-validity semantics. This mask makes
no science-quality assertion.

`Policy::Summary { accepted }` additionally requires finite WSE, a nonmissing
summary code, and membership in explicitly named documented categories.
`Policy::cookbook()` accepts good and suspect. Unknown codes are excluded.
Neither detailed flags, uncertainty nor counts silently add thresholds.

`Policy::Bitwise { require, exclude }` additionally requires finite WSE and a
nonmissing UInt32 flag word. Every required named condition must be set (AND),
and every excluded condition must be clear. Unassigned observed bits exclude
the pixel. Unknown requested names, duplicate names, contradictory lists or
unrecognized source definitions fail explicitly. A zero flag word is accepted
only when no required bits exist. UInt32 fill 4294967295 is excluded before
interpretation, and bit 31 is retained exactly without Float32 conversion.

The demo bitwise exclusion list is a user-configurable illustration; it is not
NASA's universal recommendation. `ringing` demonstrates selecting a positive
condition. File flags do not expose all numeric processing thresholds or resolve
upstream PIXC classification uncertainty. We preserve those limitations instead
of choosing undocumented cutoffs or borrowing RiverSP/vector bit definitions.

All policies return **five unchanged raw arrays**, each with its independent
source mask, and a separate derived WSE mask. Missing uncertainty/counts do not
silently remove an otherwise selected WSE. Summaries report their own available
sample counts. The mean of per-pixel one-sigma uncertainty is descriptive; it
is not uncertainty of the spatial mean and assumes no pixel independence.
No water mask, temporal alignment, height correction, outlier rejection or
river-specific aggregation is implied.
