"""E7a: install duckdb-cog (community extension) into the study's private extension dir and introspect its SQL surface.

Key question: does any function expose TileOffsets / TileByteCounts (the data a request manifest needs)?
"""

from pathlib import Path

import duckdb

from common import save

EXT = str(Path(__file__).resolve().parent / ".duckdb_ext")
con = duckdb.connect(config={"extension_directory": EXT})
con.execute("INSTALL cog FROM community")
con.execute("LOAD cog")
info = con.execute(
    "SELECT extension_name, extension_version, install_mode, installed_from FROM duckdb_extensions() WHERE extension_name='cog'"
).fetchall()
print("extension:", info)
print("cog_version():", con.execute("SELECT * FROM cog_version()").fetchall())
fns = con.execute("""SELECT function_name, function_type, parameters FROM duckdb_functions()
                     WHERE function_name ILIKE 'rs\\_%' ESCAPE '\\' OR function_name ILIKE '%cog%' OR function_name ILIKE 'read_stac%'
                     ORDER BY 1""").fetchall()
names = sorted({f[0] for f in fns})
print(len(names), "functions:", names)
suspects = [
    n
    for n in names
    if any(
        k in n.lower()
        for k in ("offset", "bytecount", "byte_count", "range", "manifest", "plan")
    )
]
print("functions mentioning offset/bytecount/range/manifest/plan:", suspects)
# schema of the tile table (use a local mock-free route: DESCRIBE needs a real file, so describe the table function signature instead)
sig = con.execute(
    "SELECT function_name, parameters, parameter_types FROM duckdb_functions() WHERE function_name IN ('read_cog','read_stac','read_stac_search')"
).fetchall()
for s in sig:
    print(s)
save(
    "e7a_cog_install",
    {
        "extension": info,
        "functions": names,
        "offset_like_functions": suspects,
        "signatures": [list(map(str, s)) for s in sig],
    },
)
