# Authenticated archive gate: design retained, native gate closed

[access.json](access.json) is a fresh test against the inspected **Version D**
object. Existing caller-owned Earthdata/netrc credentials let the reference
Requests session obtain a conditional eight-byte GET. A deliberately wrong
ETag returns 412. Final HEAD identity is rechecked. Only canonical public
locators, method, statuses, host names, lengths and opaque ETags are persisted.
Credentials, cookie values, Location headers, signed URLs and exception strings
containing authenticated URLs are excluded.

The official HEAD and GET chains terminate at the same configured CDN host,
but their method-specific signed resources are not interchangeable: GET against
the HEAD-resolved resource and HEAD against the GET-resolved resource return
403. Fresh native inspection still rejects the canonical redirect. Thus:
provider conditional ranges work; reference authentication works; **authenticated
live native acquisition is not implemented or qualified**. The capped mirror
uses the original provider ETag and size, sealed captured-extent SHA256s, and
refuses uncaptured holes. Its replay is a distinct result.

The present reference session uses the HTTP library's cookie jar and netrc
routing. It does not demonstrate a caller-configured native credential-origin
contract, adversarial redirect confinement, or safe native signed-route refresh.
Observed host chains alone cannot prove those invariants. Adding an automatic
redirect switch to the native client would not satisfy this gate.

A narrow future design would require all of the following:

1. A runtime-only caller configuration supplies the exact canonical HTTPS
   object origin/key, accepted strong ETag and length, exact authorized redirect
   origins and path patterns, redirect/hop limits, and a credential callback.
   Credential material must not enter Request serialization, explain output,
   error text or durable state. Reject userinfo, fragments, unsupported ports,
   malformed Location values, loops, unlisted hosts and every HTTPS downgrade.
   An unrestricted `*.cloudfront.net` allowlist is insufficient.
2. Resolve **each method from the canonical object**. Preserve HEAD as HEAD
   and range GET as GET across the documented 301/302/303/307/308 responses;
   reject any required unsupported method transition. Do not repurpose a signed
   HEAD URL for GET or cache signed URLs across independent operations.
3. Authorization is rebuilt only by an origin-aware credential callback.
   Earthdata Basic credentials are confined to the configured URS origin;
   archive sessions use explicitly scoped cookies. Never forward Authorization,
   Cookie, proxy credentials or URS cookies across an origin change. A secure
   cookie jar must enforce domain, path, Secure and expiry constraints. CDN
   requests carry no Earthdata credentials. Existing caller credentials must be
   read legitimately; no new account, token scraping or access-control bypass.
4. Restrict URS authorization/callback routes separately from data routes. A
   redirect to another permitted host must still satisfy the configured path
   and object-key contract. Signed query strings remain in memory only, are
   never printed, and cannot silently redefine the requested object.
5. Every final payload request sends `Range`, `If-Match` and identity encoding.
   Require exactly 206, the exact Content-Range/length, the pinned strong ETag,
   bounded body length and no content transformation. HEAD resolution must
   independently agree on ETag and total length. Revalidate the canonical
   identity before execution and after the final provisional delivery. A route
   refresh must preserve that identity; a 412 is a terminal version failure.
6. Log only method, sanitized configured host label, status, timing, byte
   counts and approved source identity. Strip Locations, request/response cookie
   headers, signed queries and URL-bearing library errors before telemetry.
   A failure path must be tested as carefully as the success path.
7. First prove local adversarial tests for credential exfiltration to a second
   host, unsafe redirects/ports, downgrade, loops, method confusion, expired
   routes, malformed ranges, transformed/truncated bodies, identity mismatch,
   cancellation and final-source mutation. Then run a capped real D native
   acquisition of all five layers and compare against the independent readers.

These invariants have not been demonstrated in a native implementation during
this milestone. No HTTP validation was weakened. The existing transport stays
unchanged, and the next useful provider-access milestone is this explicit
caller-configured authentication/redirect contract shared across formats.
