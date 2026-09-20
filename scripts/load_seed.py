#!/usr/bin/env python
"""把契约夹具的 seed（Django `dumpdata` 输出）灌进 scratch schema 的契约表。

为什么能直接灌
--------------
`navel_backend_git/api/models.py` 里 30 个模型**全部**显式声明了 `db_table`，且字段名与我们
的列名逐字一致（外键列是 `<field>_id`）。所以 Django 的 dumpdata 记录可以几乎原样落库，
**主键（UUID）也保持一致** —— 这让回放比对不必屏蔽 id，比"忽略 ID"强得多。

实现方式
--------
不引入 psycopg：生成 INSERT SQL 文件，交给 `psql -f` 执行（`load_seed.py` 只依赖 stdlib）。

安全
----
- 只接受 `compat_*` schema；`public` 被明确拒绝（现网 `app_*`/`store_*`/`commerce_*` 有真实数据）。
- 表名/列名一律来自目标 schema 的 `information_schema`，不靠猜。
- 字段名在本表里既没有 `<field>` 也没有 `<field>_id` 列时**报错退出**，不静默丢字段。

用法
----
    python db\\scripts\\load_seed.py --schema compat_test
    python db\\scripts\\load_seed.py --schema compat_test --dry-run
    python db\\scripts\\load_seed.py --schema compat_test --seed <其它 seed.json>
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

DB_ROOT = Path(__file__).resolve().parent.parent
REPO_ROOT = DB_ROOT.parent
DEFAULT_SEED = DB_ROOT / "tests" / "fixtures" / "contract" / "seed.json"
CONFIG = DB_ROOT / "config.toml"

# Django 模型 label -> 我方契约表名（对应 models.py 的 Meta.db_table）。
MODEL_TABLE = {
    "api.homedata": "home_data",
    "api.growthtracking": "growth_tracking",
    "api.diagnosedata": "diagnose_data",
    "api.diagnoselistitem": "diagnose_list_items",
    "api.user": "user",
    "api.temperaturehumiditydata": "temperature_humidity_data",
    "api.task": "task",
    "api.diseaserecognitionrecord": "disease_recognition_record",
    "api.authtoken": "auth_token",
    "api.orchard": "orchard",
    "api.fruittreearchive": "fruit_tree_archive",
    "api.salesbatch": "sales_batch",
    "api.harvestarchive": "harvest_archive",
    "api.batchqualitysample": "batch_quality_sample",
    "api.traceevent": "trace_event",
    "api.citrusproduct": "citrus_product",
    "api.cartitem": "cart_item",
    "api.buyeraddress": "buyer_address",
    "api.order": "order",
    "api.orderitem": "order_item",
    "api.paymentrecord": "payment_record",
    "api.tracepackage": "trace_package",
    "api.aftersalerequest": "after_sale_request",
    "api.agentapproval": "agent_approval",
    "api.agentfeedback": "agent_feedback",
}

# 依赖顺序：被引用的表先插（外键是 DEFERRABLE，同事务内其实乱序也成立，但先排好更利定位）。
TABLE_ORDER = [
    "user",
    "auth_token",
    "home_data",
    "growth_tracking",
    "diagnose_data",
    "diagnose_list_items",
    "temperature_humidity_data",
    "task",
    "disease_recognition_record",
    "orchard",
    "fruit_tree_archive",
    "sales_batch",
    "harvest_archive",
    "batch_quality_sample",
    "trace_event",
    "citrus_product",
    "cart_item",
    "buyer_address",
    "order",
    "order_item",
    "payment_record",
    "trace_package",
    "after_sale_request",
    "agent_approval",
    "agent_feedback",
]

NUMERIC_TYPES = {"smallint", "integer", "bigint", "real", "double precision", "numeric"}
SIMPLE_TEXT_TYPES = {"text", "character varying", "character"}
CAST_TYPES = {
    "uuid": "uuid",
    "timestamp with time zone": "timestamptz",
    "timestamp without time zone": "timestamp",
    "date": "date",
    "numeric": "numeric",
    "jsonb": "jsonb",
    "json": "jsonb",
}


# --------------------------------------------------------------------------------------
# 环境
# --------------------------------------------------------------------------------------


def find_pg_tool(name: str) -> str:
    import shutil

    found = shutil.which(name)
    if found:
        return found
    for cand in (
        Path(r"D:\apps\pg\18\bin") / f"{name}.exe",
        Path(r"C:\Program Files\PostgreSQL\18\bin") / f"{name}.exe",
    ):
        if cand.exists():
            return str(cand)
    raise SystemExit(f"找不到 {name}：请放进 PATH，或设置 PG_BIN")


def base_dsn() -> str:
    text = CONFIG.read_text(encoding="utf-8")
    m = re.search(r'(?m)^\s*postgres_url\s*=\s*"([^"]+)"', text)
    if not m:
        raise SystemExit(f"{CONFIG} 里没有 [database].postgres_url")
    return m.group(1)


def assert_compat_schema(schema: str) -> None:
    if schema == "public":
        raise SystemExit("拒绝操作 public：现网 app_*/store_*/commerce_* 有真实数据。")
    if not re.fullmatch(r"compat_[a-z0-9_]+", schema):
        raise SystemExit(f"非法 schema 名 {schema!r}：只接受 ^compat_[a-z0-9_]+$")


def schema_dsn(dsn: str, schema: str) -> str:
    opt = "options=-csearch_path%3D" + schema
    return f"{dsn}&{opt}" if "?" in dsn else f"{dsn}?{opt}"


def run_psql(dsn: str, args: list[str], sql: str | None = None) -> str:
    cmd = [find_pg_tool("psql"), dsn, "-v", "ON_ERROR_STOP=1", "-q", *args]
    proc = subprocess.run(cmd, input=sql, capture_output=True, text=True, encoding="utf-8")
    if proc.returncode != 0:
        raise SystemExit(f"psql 失败（exit {proc.returncode}）:\n{proc.stderr or proc.stdout}")
    return proc.stdout


# --------------------------------------------------------------------------------------
# 目标 schema 内省
# --------------------------------------------------------------------------------------


def introspect(dsn: str, schema: str, tables: list[str]) -> tuple[dict, dict[str, str]]:
    """返回 ({表: {列: 类型}}, {表: 主键列})。"""
    literal = ", ".join("'" + t.replace("'", "''") + "'" for t in tables)
    rows = run_psql(
        dsn,
        ["-At", "-F", "\t", "-c",
         "select table_name, column_name, data_type from information_schema.columns "
         f"where table_schema = '{schema}' and table_name in ({literal}) "
         "order by table_name, ordinal_position"],
    )
    columns: dict[str, dict[str, str]] = {}
    for line in rows.splitlines():
        if not line.strip():
            continue
        table, column, dtype = line.split("\t")
        columns.setdefault(table, {})[column] = dtype

    pk_rows = run_psql(
        dsn,
        ["-At", "-F", "\t", "-c",
         "select kcu.table_name, kcu.column_name "
         "from information_schema.table_constraints tc "
         "join information_schema.key_column_usage kcu "
         "  on kcu.constraint_name = tc.constraint_name "
         " and kcu.table_schema = tc.table_schema "
         f"where tc.table_schema = '{schema}' and tc.constraint_type = 'PRIMARY KEY'"],
    )
    pks: dict[str, str] = {}
    for line in pk_rows.splitlines():
        if not line.strip():
            continue
        table, column = line.split("\t")
        pks.setdefault(table, column)

    missing = [t for t in tables if t not in columns]
    if missing:
        raise SystemExit(
            f"目标 schema {schema} 里缺这些表：{missing}\n"
            f"先跑：.\\pg_env.ps1 -Reset {schema} ; .\\pg_env.ps1 -Apply {schema}"
        )
    return columns, pks


# --------------------------------------------------------------------------------------
# 值渲染
# --------------------------------------------------------------------------------------


def quote_literal(value: str) -> str:
    # standard_conforming_strings=on（PG 默认）下反斜杠是字面量，只需转义单引号。
    return "'" + value.replace("'", "''") + "'"


def render(value, dtype: str) -> str:
    if value is None:
        return "NULL"
    if dtype == "boolean":
        return "TRUE" if value else "FALSE"
    if dtype == "jsonb":
        payload = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        return quote_literal(payload) + "::jsonb"
    if dtype in NUMERIC_TYPES:
        return f"{value}::{dtype}"
    if dtype == "uuid":
        return quote_literal(str(value)) + "::uuid"
    if dtype in CAST_TYPES:
        return quote_literal(str(value)) + "::" + CAST_TYPES[dtype]
    if dtype in SIMPLE_TEXT_TYPES:
        return quote_literal(str(value))
    # 未预期类型：按文本塞进去，让 PG 自己报错，好过静默错值。
    return quote_literal(str(value))


def build_rows(objects, columns, pks):
    """把 seed objects 按表聚成 INSERT 语句，返回 (语句列表, 计数, 未映射字段列表)。"""
    by_table: dict[str, list[str]] = {}
    counts: dict[str, int] = {}
    unmapped: list[str] = []

    for obj in objects:
        model = obj["model"]
        table = MODEL_TABLE.get(model)
        if table is None:
            raise SystemExit(f"seed 里出现未知模型 {model!r}，请更新 MODEL_TABLE 映射")
        table_columns = columns[table]
        pk_column = pks[table]

        names: list[str] = []
        values: list[str] = []

        names.append(pk_column)
        if obj["pk"] is None:
            values.append("NULL")
        else:
            values.append(render(obj["pk"], table_columns[pk_column]))

        for field, value in obj["fields"].items():
            if field in table_columns:
                column = field
            elif f"{field}_id" in table_columns:
                column = f"{field}_id"
            else:
                unmapped.append(f"{model}.{field} -> {table}")
                continue
            names.append(column)
            values.append(render(value, table_columns[column]))

        stmt = (
            f"INSERT INTO \"{table}\" ({', '.join('\"' + n + '\"' for n in names)}) "
            f"VALUES ({', '.join(values)});"
        )
        by_table.setdefault(table, []).append(stmt)
        counts[table] = counts.get(table, 0) + 1

    statements: list[str] = []
    for table in TABLE_ORDER:
        statements.extend(by_table.get(table, []))
    for table in sorted(set(by_table) - set(TABLE_ORDER)):
        statements.extend(by_table[table])
    return statements, counts, unmapped


# --------------------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(description="把契约 seed.json 灌进 scratch schema")
    ap.add_argument("--seed", default=str(DEFAULT_SEED), help="seed.json 路径")
    ap.add_argument("--schema", default="compat_test", help="目标 scratch schema")
    ap.add_argument("--dsn", default=None, help="覆盖 config.toml 的连接串")
    ap.add_argument("--no-truncate", action="store_true", help="不先清空目标表")
    ap.add_argument("--dry-run", action="store_true", help="只生成 SQL 不执行")
    args = ap.parse_args()

    assert_compat_schema(args.schema)
    seed_path = Path(args.seed)
    if not seed_path.exists():
        raise SystemExit(f"找不到 seed：{seed_path}")

    payload = json.loads(seed_path.read_text(encoding="utf-8"))
    objects = payload["objects"]
    meta = payload.get("_meta", {})

    dsn = schema_dsn(args.dsn or base_dsn(), args.schema)
    tables = list(MODEL_TABLE.values())
    columns, pks = introspect(dsn, args.schema, tables)
    statements, counts, unmapped = build_rows(objects, columns, pks)

    if unmapped:
        print("!! 以下字段在目标表里既没有同名列也没有 <field>_id 列：")
        for item in sorted(set(unmapped)):
            print("   ", item)
        return 1

    header = [
        f"SET search_path TO \"{args.schema}\";",
        "BEGIN;",
    ]
    if not args.no_truncate:
        quoted = ", ".join(f'"{t}"' for t in tables)
        header.append(f"TRUNCATE {quoted} RESTART IDENTITY CASCADE;")
    footer = ["COMMIT;"]

    sql = "\n".join(header) + "\n" + "\n".join(statements) + "\n" + "\n".join(footer) + "\n"
    sql_path = Path(tempfile.gettempdir()) / f"seed_load_{args.schema}.sql"
    sql_path.write_text(sql, encoding="utf-8", newline="\n")

    print(f"[seed] {seed_path}")
    print(f"[seed] captured_at={meta.get('captured_at')} total_rows={meta.get('total_rows')}")
    print(f"[seed] schema={args.schema} -> {len(statements)} 条 INSERT 语句")
    print(f"[seed] SQL: {sql_path}")

    if args.dry_run:
        print("[seed] --dry-run：未执行")
        return 0

    run_psql(dsn, ["-f", str(sql_path)])

    actual = run_psql(
        dsn,
        ["-At", "-F", "\t", "-c",
         "select relname, n_live_tup from pg_stat_user_tables "
         f"where schemaname = '{args.schema}' order by relname"],
    )
    live = {}
    for line in actual.splitlines():
        if line.strip():
            name, n = line.split("\t")
            live[name] = int(n)

    print("\n=== 每表行数（预期 / 实测） ===")
    total_expected = 0
    for table in TABLE_ORDER:
        expected = counts.get(table, 0)
        total_expected += expected
        got = live.get(table, 0)
        flag = "ok " if got == expected else "!! "
        print(f"  {flag}{table:<28} {expected:>3} / {got:>3}")
    print(f"  合计 {total_expected} 行")

    mismatch = [t for t in TABLE_ORDER if live.get(t, 0) != counts.get(t, 0)]
    if mismatch:
        print(f"\n!! 行数不符：{mismatch}")
        return 1
    print("\n[ok] seed 加载完成，行数全部一致")
    return 0


if __name__ == "__main__":
    sys.exit(main())
