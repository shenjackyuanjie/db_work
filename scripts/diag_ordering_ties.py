"""D12 侦察：定位列表「平局」是在种子环节还是录制环节产生的。

用法：
    python db\\scripts\\diag_ordering_ties.py            # 只看夹具与种子文件
    python db\\scripts\\diag_ordering_ties.py --schema compat_test   # 顺带查 PG 里的实际顺序
"""

import argparse
import json
import pathlib
import subprocess
import sys

FIX = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "contract"
PSQL = r"D:\apps\pg\18\bin\psql.exe"
DSN = "postgres://db_race:datarace@192.168.3.52:5400/db_race"


def load(name):
    return json.loads((FIX / name).read_text(encoding="utf-8"))


def seed_rows(seed, model):
    return [o for o in seed["objects"] if o["model"] == model]


def show_seed(seed, model, field="created_at"):
    rows = seed_rows(seed, model)
    print(f"\n--- seed.json: {model}（{len(rows)} 行）按 {field} 排序 ---")
    for obj in sorted(rows, key=lambda o: o["fields"].get(field) or ""):
        fields = obj["fields"]
        label = fields.get("title") or fields.get("note") or fields.get("product_name") or ""
        print(f"  pk={obj['pk']}  {field}={fields.get(field)}  {label[:28]}")


def show_case(domain, case_name):
    doc = load(f"{domain}.json")
    case = next((c for c in doc["cases"] if c["name"] == case_name), None)
    if case is None:
        print(f"\n!! 找不到用例 {domain}/{case_name}")
        return
    print(f"\n--- {domain}.json :: {case_name} 的期望顺序 ---")
    body = case.get("expected_body")
    if body is None:
        print("  （无 expected_body）")
        return
    print(f"  normalize={case.get('normalize')}")
    data = body.get("data")
    rows = data if isinstance(data, list) else (data.get("tasks") or data.get("risk_cards") or [])
    for idx, row in enumerate(rows):
        if not isinstance(row, dict):
            print(f"  [{idx}] {row}")
            continue
        ident = row.get("id") or row.get("title") or row
        print(f"  [{idx}] id={row.get('id')}  created_at={row.get('created_at')}  "
              f"title/note={str(row.get('title') or row.get('note') or '')[:26]}  "
              f"inner={('evidence' in json.dumps(row, ensure_ascii=False))}")
        del ident


def pg_query(schema, sql):
    proc = subprocess.run(
        [PSQL, DSN, "-v", "ON_ERROR_STOP=1", "-At", "-c", f"SET search_path TO {schema}; {sql}"],
        capture_output=True, text=True, encoding="utf-8",
    )
    if proc.returncode != 0:
        print(f"  !! psql 失败: {proc.stderr.strip()}")
        return []
    return [line for line in proc.stdout.splitlines() if line.strip()]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--schema", default=None, help="顺带查这个 schema 里 PG 的实际返回顺序")
    args = parser.parse_args()

    seed = load("seed.json")
    print("=== 一、种子里的平局情况 ===")
    show_seed(seed, "api.task")
    show_seed(seed, "api.agentfeedback")

    print("\n=== 二、夹具期望的顺序 ===")
    show_case("core", "tasks_list_ok")
    show_case("agent", "agent_context_ok")
    show_case("agent", "agent_feedback_get_buyer_ok")

    if args.schema:
        print(f"\n=== 三、PG({args.schema}) 实际返回顺序 ===")
        print("  -- app 侧按 created_at DESC, id --")
        for line in pg_query(args.schema, "SELECT id, created_at FROM task ORDER BY created_at DESC;"):
            print(f"    {line}")
        print("  -- 蓝本 Meta.ordering 等价（仅 created_at DESC，无次级键）--")
        for line in pg_query(args.schema, "SELECT id, created_at FROM task ORDER BY created_at DESC, ctid;"):
            print(f"    {line}")


if __name__ == "__main__":
    sys.exit(main())
