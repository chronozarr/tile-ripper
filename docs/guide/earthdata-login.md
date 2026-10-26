# Earthdata login

Some NASA data can only be downloaded with a free NASA Earthdata login.
Tile-Ripper reads your username and password from a private file that
you name on the command line (a netrc file). It reads that file only
when you ask it to, and it never looks for credentials anywhere else.

## Which data need it

[Supported data](supported-data.md) lists the account needed for every
dataset. The ones that need an Earthdata login are:

| Dataset | Archive |
| --- | --- |
| SWOT L2 HR Raster version D, release PGD0 (composite release identifier, CRID) | PO.DAAC |
| SMAP SPL3SMP_E version 6, global and northern | NSIDC |
| NISAR L2 GCOV (geocoded covariance) provisional, frequency A HHHH (horizontal transmit and receive) | ASF |
| OPERA DSWx-HLS v1.1, OPERA DSWx-S1 | PO.DAAC |
| OPERA RTC-S1 (current production data) | ASF |
| OPERA DIST-ALERT-HLS, HLS L30 and S30 (NASA production archive) | LP DAAC |

Sentinel-2, Landsat, GOES, the Zarr stores (ERA5, IMERG, GFS) and every
catalog `discover` search need no account. Landsat and HLS can also be
read through Microsoft Planetary Computer without a login.

## Set up

**Step 1.** Create a free account at <https://urs.earthdata.nasa.gov>.

**Step 2.** Create a private netrc file, then open it in a text editor.
Typing the password in an editor keeps it out of your shell history.

```sh
touch "$HOME/.netrc"
chmod 600 "$HOME/.netrc"
```

**Step 3.** Add one line, with your own username and password:

```text
machine urs.earthdata.nasa.gov login USERNAME password PASSWORD
```

The file must meet these rules:

- It is a regular file that you own. A symlink is refused.
- Neither your group nor other users may have any permission on it,
  which is what `chmod 600` sets.
- Words are separated by spaces or new lines. A line starting with `#`
  is a comment. Quote a password that has spaces or starts with `#`,
  with `"..."` or `'...'`. A backslash escapes the next character.
- Use only `machine`, `login`, `password` and `account`. Anything else,
  such as `macdef` or `port`, is rejected. Tile-Ripper uses the
  `urs.earthdata.nasa.gov` entry and ignores `default` entries.
- At most 64 KiB. The username cannot contain `:`, and control
  characters are rejected.
- macOS and Linux only. Other systems cannot pass the ownership check.

If your `~/.netrc` holds other entries that break these rules, put the
Earthdata line in a separate file and pass that path to `--netrc`.

## Use it

Add both flags to `plan`, `fetch`, `stream` or `discover-job`:

```sh
tile-ripper plan examples/product-smap.json --gdal-free \
  --auth earthdata --netrc "$HOME/.netrc"
```

- **Opt-in only.** The file is read only when you pass
  `--auth earthdata --netrc "$HOME/.netrc"` (or another path). Tile-Ripper never reads `.netrc`,
  `NETRC` or `TILE_RIPPER_AUTH_NETRC` on its own, and `discover` accepts
  no authentication flags.
- **One destination.** Your credentials are sent only to
  `urs.earthdata.nasa.gov`. They are never written to plans, manifests,
  outputs, job files or checkpoints.
- **Keep them out of requests.** Do not put credentials in request
  JSON, command arguments or logs, and do not commit the netrc file.

## Troubleshooting

Errors print as `tile-ripper: MESSAGE`. The messages below come from
the netrc reader, which checks your file before any network request.

| Message | Cause | Fix |
| --- | --- | --- |
| `cannot open private netrc credential source` | The path does not exist, is unreadable, or is a symlink. | Check the `--netrc` path. Use the real file, not a link. |
| `netrc credential source must be private and owned by the caller` | Group or other users have permissions, or you do not own the file. | Run `chmod 600 "$HOME/.netrc"`. Make sure you own the file. |
| `unsupported netrc credential syntax` | A keyword other than `machine`, `login`, `password`, `account` or `default` appears before or inside the Earthdata entry (`macdef`, `port`). | Delete it, or use a separate file with only the Earthdata line. |
| `netrc has no explicit credentials for the configured login host` | There is no `machine urs.earthdata.nasa.gov` entry with both `login` and `password`. The file may be empty, or hold only comments or `default`. | Add the line exactly as shown above. |
| `invalid netrc credential syntax` | `login` or `password` is the last word, with no value. | Add the missing value. |
| `invalid netrc credential characters` | The username contains `:`, or a value has a control character. | Re-type the entry. Check for pasted control characters. |
| `invalid netrc quoted value` | A quote was opened and never closed. | Close the quote. |
| `invalid netrc escape` | The file ends with a backslash. | Remove it or escape it. |
| `invalid netrc credential encoding` | The file is not valid UTF-8 text. | Save it as plain UTF-8. |
| `invalid bounded netrc credential source` | The path is a directory or not a regular file, or the file is over 64 KiB. | Point `--netrc` at a small regular file. |
| `netrc credential source exceeds bounds` or `netrc token budget exceeded` | The file is too large or has more than 1,024 words. | Use a small file with only the Earthdata line. |
| `private netrc authentication currently requires Unix ownership and permission checks` | You are not on macOS or Linux. | Use macOS or Linux. |
| `authentication requires explicit --auth and --netrc; credential environment variables are not used` | One of the two flags is missing. | Pass both: `--auth earthdata --netrc "$HOME/.netrc"`. |
| `live SMAP requires explicit --auth earthdata --netrc PRIVATE_FILE` (also `live NISAR ...`) | SMAP or NISAR was run without the flags. | Add `--auth earthdata --netrc "$HOME/.netrc"`. |
| `HTTP object request rejected (unsupported status or redirect)` on a SWOT job | Most often, the job was run without the flags and the protected archive refused the anonymous request. | Add `--auth earthdata --netrc "$HOME/.netrc"`. |
| `this array product has no qualified Earthdata route` | The flags were passed for GOES, which needs no login. | Remove `--auth` and `--netrc`. |
| `authenticated access rejected; refresh exhausted` | NASA answered with HTTP 401 or 403 even after a retry. | Check the username and password in the file by signing in at the Earthdata website. Then see the note below. |

A SWOT job also fails with `authenticated object identity changed` if
the archive file differs from the version (ETag) pinned in the job. Run
the search again to get the current file, or update the ETag in the job
file (`etag`, or `expected_etag` in a selection file).

### A note on data-use agreements

This is general NASA Earthdata guidance, not Tile-Ripper behavior. Some
NASA archives require you to accept a data-use agreement, or to
authorize an application, on the Earthdata website once. If your login
works on the website but a download still fails with a 401 or 403 (for
example `authenticated access rejected; refresh exhausted`), check your
Earthdata profile for an approval that the dataset needs.

## Details and evidence

- [Authenticated native acquisition](../earthdata-auth/workflow.md)
- [Provider routing](../providers/qualification.md) and
  [SWOT jobs](../swot-job/workflow.md)
- [Transfer resume](../transfer-resume/workflow.md) and the
  [support contract](../alpha/contract.md)
