"""核对 index.json 的元数据与实际录制用例数是否自洽（case_count 246 vs 各域合计 251）。

口径说明：
- 各域 `*.json` 的 `case_count` = 该域**实际录制**的用例数
- `index.json` 的 `case_count` = 各 path 条目按 `(url_name, method)` 归集后的用例数之和

用法：
    python db\\scripts\\diag_case_count.py
"""

import json
import pathlib

FIX = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "contract"
DOMAINS = ["auth", "core", "commerce", "orchard_trace", "agent"]


def main():
    per_domain = {}
    flat = []
    cases_by_name = {}
    for domain in DOMAINS:
        doc = json.loads((FIX / f"{domain}.json").read_text(encoding="utf-8"))
        names = [c["name"] for c in doc["cases"]]
        per_domain[domain] = len(names)
        flat.extend(names)
        for case in doc["cases"]:
            cases_by_name[case["name"]] = (domain, case.get("path_name"), case.get("method"))

    index = json.loads((FIX / "index.json").read_text(encoding="utf-8"))

    print("=== 实际录制用例数（各域文件）===")
    for domain, count in per_domain.items():
        print(f"  {domain:<15} {count}")
    print(f"  {'合计':<14} {len(flat)}")
    print(f"\nindex.case_count = {index.get('case_count')}")
    print(f"index.path_count = {index.get('path_count')}")
    print(f"index.mutation_order 条数 = {len(index.get('mutation_order') or [])}")

    # path 条目里逐个登记的用例名
    registered = []
    for entry in index.get("paths", []) or []:
        for per_method in entry.get("method_case_counts", []) or []:
            registered.extend(per_method.get("cases") or [])

    per_path_sum = sum(e.get("cases", 0) for e in index.get("paths", []) or [])
    print(f"\n各 path 条目 cases 之和 = {per_path_sum}")
    print(f"path 条目登记的用例名数 = {len(registered)}，去重后 = {len(set(registered))}")

    flat_set, reg_set = set(flat), set(registered)
    extra = [n for n in flat if n not in reg_set]
    missing = [n for n in registered if n not in flat_set]

    print(f"\n=== 只在域文件里、index 的 path 条目未收录（{len(extra)}）===")
    for name in extra:
        domain, path_name, method = cases_by_name.get(name, ("?", "?", "?"))
        print(f"  {domain:<15} {method or '?':<7} {path_name or '?':<34} {name}")

    print(f"\n=== 只在 index 里、域文件没有（{len(missing)}）===")
    for name in missing:
        print(f"  {name}")


if __name__ == "__main__":
    main()
