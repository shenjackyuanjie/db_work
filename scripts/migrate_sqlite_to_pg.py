#!/usr/bin/env python3
"""Migrate legacy Django SQLite data into Rust service PostgreSQL tables.

Source (SQLite):
- user
- task
- temperature_humidity_data
- disease_recognition_record

Target (PostgreSQL):
- app_users
- app_tasks
- app_temperature_humidity
- app_diagnosis_records

Notes:
- app_users.password_hash stores legacy Django hash as-is. Rust login currently expects blake3,
  so migrated users should reset password or you should add Django-hash compatibility in login.
"""

from __future__ import annotations

import argparse
import datetime as dt
import sqlite3
from pathlib import Path
from typing import Any

import psycopg


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Migrate SQLite data to PostgreSQL")
    p.add_argument(
        "--sqlite",
        default="../navel_back/db.sqlite3",
        help="Path to source SQLite database",
    )
    p.add_argument(
        "--pg-url",
        required=True,
        help="PostgreSQL connection URL, e.g. postgres://user:pass@host:port/db",
    )
    p.add_argument(
        "--dry-run",
        action="store_true",
        help="Only print counts; do not write data",
    )
    return p.parse_args()


def parse_datetime(value: Any) -> dt.datetime | None:
    if value is None:
        return None
    text = str(value).strip()
    if not text:
        return None

    candidates = [
        "%Y-%m-%d %H:%M:%S.%f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
    ]
    for fmt in candidates:
        try:
            return dt.datetime.strptime(text, fmt)
        except ValueError:
            continue

    try:
        return dt.datetime.fromisoformat(text)
    except ValueError:
        return None


def to_unix_seconds(value: Any) -> int:
    parsed = parse_datetime(value)
    if parsed is None:
        return 0
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return int(parsed.timestamp())


def to_unix_millis(value: Any) -> int:
    parsed = parse_datetime(value)
    if parsed is None:
        return 0
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return int(parsed.timestamp() * 1000)


def infer_is_healthy(disease_name: str | None) -> bool:
    name = (disease_name or "").strip()
    return name == "健康果树"


def infer_is_citrus_leaf(disease_name: str | None) -> bool:
    name = (disease_name or "").strip()
    return name not in {"", "非果树"}


def infer_severity(risk_level: str | None, disease_name: str | None) -> str:
    if infer_is_healthy(disease_name):
        return "健康"
    level = (risk_level or "").strip()
    if level == "高风险":
        return "重度"
    if level == "中风险":
        return "中度"
    return "轻度"


def main() -> None:
    args = parse_args()

    sqlite_path = Path(args.sqlite)
    if not sqlite_path.exists():
        raise SystemExit(f"SQLite file not found: {sqlite_path}")

    src = sqlite3.connect(str(sqlite_path))
    src.row_factory = sqlite3.Row

    with psycopg.connect(args.pg_url) as pg_conn:
        with pg_conn.cursor() as cur:
            # Build user-id -> username map for joins
            users = src.execute(
                "SELECT id, username, password, created_at FROM user"
            ).fetchall()
            user_id_to_name = {row["id"]: row["username"] for row in users}

            tasks = src.execute(
                "SELECT id, title, description, risk_level, task_type, source, is_completed, "
                "created_at, completed_at, user_id FROM task"
            ).fetchall()

            temp_rows = src.execute(
                "SELECT id, timestamp, temperature, humidity, user_id FROM temperature_humidity_data"
            ).fetchall()

            diag_rows = src.execute(
                "SELECT id, disease_name, area, risk_level, confidence, created_at, user_id "
                "FROM disease_recognition_record"
            ).fetchall()

            print(f"Found users={len(users)} tasks={len(tasks)} temp={len(temp_rows)} diag={len(diag_rows)}")
            if args.dry_run:
                print("Dry-run only, no data written.")
                return

            # users
            user_written = 0
            for row in users:
                username = row["username"]
                password_hash = row["password"] or ""
                created_at = to_unix_seconds(row["created_at"])

                cur.execute(
                    """
                    INSERT INTO app_users (username, password_hash, is_admin, created_at, session_token)
                    VALUES (%s, %s, %s, %s, NULL)
                    ON CONFLICT (username) DO NOTHING
                    """,
                    (username, password_hash, False, created_at),
                )
                user_written += cur.rowcount

            # tasks
            task_written = 0
            for row in tasks:
                username = user_id_to_name.get(row["user_id"]) if row["user_id"] else None
                if not username:
                    continue

                created_at = to_unix_millis(row["created_at"])
                completed_at = to_unix_millis(row["completed_at"]) if row["completed_at"] else None
                is_completed = bool(row["is_completed"])

                cur.execute(
                    """
                    INSERT INTO app_tasks
                    (id, username, title, description, risk_level, task_type, source, is_completed, created_at, completed_at)
                    VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
                    ON CONFLICT (id) DO NOTHING
                    """,
                    (
                        str(row["id"]),
                        username,
                        row["title"] or "",
                        row["description"] or "",
                        row["risk_level"] or "中风险",
                        row["task_type"] or "手动添加",
                        row["source"] or "用户",
                        is_completed,
                        created_at,
                        completed_at,
                    ),
                )
                task_written += cur.rowcount

            # temperature/humidity
            temp_written = 0
            for row in temp_rows:
                username = user_id_to_name.get(row["user_id"]) if row["user_id"] else None
                ts = to_unix_millis(row["timestamp"])

                cur.execute(
                    """
                    INSERT INTO app_temperature_humidity (username, timestamp, temperature, humidity)
                    VALUES (%s, %s, %s, %s)
                    """,
                    (
                        username,
                        ts,
                        float(row["temperature"] or 0.0),
                        float(row["humidity"] or 0.0),
                    ),
                )
                temp_written += 1

            # diagnosis records
            diag_written = 0
            for row in diag_rows:
                disease_name = (row["disease_name"] or "").strip()
                username = user_id_to_name.get(row["user_id"]) if row["user_id"] else None
                ts = to_unix_millis(row["created_at"])

                cur.execute(
                    """
                    INSERT INTO app_diagnosis_records
                    (id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy,
                     disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning,
                     username, area)
                    VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
                    ON CONFLICT (id) DO NOTHING
                    """,
                    (
                        str(row["id"]),
                        ts,
                        disease_name or "健康果树",
                        float(row["confidence"] or 0.0),
                        infer_is_citrus_leaf(disease_name),
                        "其他",
                        infer_is_healthy(disease_name),
                        "" if infer_is_healthy(disease_name) else disease_name,
                        infer_severity(row["risk_level"], disease_name),
                        "",
                        "",
                        "",
                        username,
                        row["area"] or "未指定区域",
                    ),
                )
                diag_written += cur.rowcount

            pg_conn.commit()

    src.close()

    print("Migration done:")
    print(f"  app_users inserted: {user_written}")
    print(f"  app_tasks inserted: {task_written}")
    print(f"  app_temperature_humidity inserted: {temp_written}")
    print(f"  app_diagnosis_records inserted: {diag_written}")
    print(
        "Note: migrated password_hash is Django format; Rust login expects blake3, so old users need password reset or login compatibility patch."
    )


if __name__ == "__main__":
    main()
