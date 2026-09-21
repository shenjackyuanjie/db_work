"""裁定两套契约夹具：实测用例数 / normalize 缺失 / 未屏蔽 UUID / seed 形态 / index 统计字段。"""

import json
import pathlib
import re
import sys

UUID_RE = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", re.I)
SKIP = {"index.json", "seed.json", "probes.json"}


def walk(node, path="$"):
    if isinstance(node, dict):
        for key, value in node.items():
            yield from walk(value, f"{path}.{key}")
    elif isinstance(node, list):
        for value in node:
            yield from walk(value, f"{path}[*]")
    else:
        yield path, node


def seed_rows(doc):
    body = doc.get("objects") if isinstance(doc, dict) else doc
    if isinstance(body, list):
        return len(body)
    if isinstance(body, dict) and isinstance(body.get("objects"), list):
        return len(body["objects"])
    return -1


for base in sys.argv[1:]:
    root = pathlib.Path(base)
    print("=" * 24, root)
    if not root.exists():
        print("  MISSING")
        continue

    total = empty_norm = shape = uuid_hits = 0
    for path in sorted(root.glob("*.json")):
        if path.name in SKIP:
            continue
        try:
            doc = json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:  # noqa: BLE001
            print(f"  PARSE-FAIL {path.name}: {exc}")
            continue
        cases = doc.get("cases") if isinstance(doc, dict) else doc
        if not isinstance(cases, list):
            cases = []
        for case in cases:
            total += 1
            if case.get("shape_only"):
                shape += 1
            if not case.get("normalize"):
                empty_norm += 1
            for _, value in walk(case.get("expected_body")):
                if isinstance(value, str) and UUID_RE.search(value):
                    uuid_hits += 1
        print(f"  {path.name:<22} cases={len(cases)}")

    print(f"  -> cases={total} empty_normalize={empty_norm} shape_only={shape} uuid_in_body={uuid_hits}")

    seed_path = root / "seed.json"
    if seed_path.exists():
        doc = json.loads(seed_path.read_text(encoding="utf-8"))
        print(f"  seed.json rows={seed_rows(doc)} top_keys={list(doc)[:8] if isinstance(doc, dict) else 'LIST'}")

    index_path = root / "index.json"
    if index_path.exists():
        doc = json.loads(index_path.read_text(encoding="utf-8"))
        keys = list(doc) if isinstance(doc, dict) else []
        wanted = ["covered", "partial", "blocked", "uncovered", "mutation_order", "alias_conclusion", "unresolved"]
        print(f"  index.json keys={keys}")
        print(f"  index.json 缺失字段={[k for k in wanted if k not in keys]}")
