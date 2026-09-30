import json
from pathlib import Path

for name in ("re/traces/native.jsonl", "re/traces/port_c.jsonl"):
    print("====", name)
    for line in Path(name).read_text(encoding="utf-8").splitlines():
        row = json.loads(line)
        if row.get("kind") == "capture":
            print("  head:", {k: v for k, v in row.items() if k != "kind"})
            continue
        body = bytes.fromhex(row.get("hex") or "")
        print(f"  {row['seq']:>3} {row['dir']:>4} id={row['id']:<4} {row['name']:<18} "
              f"len={row['len']:<6} body={body[:16].hex()}")
