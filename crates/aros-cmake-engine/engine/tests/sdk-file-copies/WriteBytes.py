from pathlib import Path
import sys

if len(sys.argv) != 3:
    raise SystemExit("usage: WriteBytes.py OUTPUT HEX_BYTES")

Path(sys.argv[1]).write_bytes(bytes.fromhex(sys.argv[2]))
