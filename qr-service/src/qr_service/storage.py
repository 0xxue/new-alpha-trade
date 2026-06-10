"""SQLite 持久化层。

复用 trading-engine 创建的同一个数据库（trading-engine 跑 migration，
qr-service 只读写 accounts 表）。如果数据库不存在，qr-service 启动时会建空文件，
但不会主动建表 —— 让 Rust 那边的 sqlx migrate 来负责 schema。
"""
from __future__ import annotations

import json
from contextlib import asynccontextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import AsyncIterator

import aiosqlite


@dataclass
class Account:
    username: str
    cookies: dict[str, str]
    headers: dict[str, str]
    twofa_secret: str | None
    last_refresh: str | None
    status: str


class Storage:
    def __init__(self, db_path: Path) -> None:
        self.db_path = db_path
        self.db_path.parent.mkdir(parents=True, exist_ok=True)

    @asynccontextmanager
    async def _conn(self) -> AsyncIterator[aiosqlite.Connection]:
        db = await aiosqlite.connect(self.db_path)
        try:
            await db.execute("PRAGMA foreign_keys = ON")
            await db.execute("PRAGMA journal_mode = WAL")
            db.row_factory = aiosqlite.Row
            yield db
            await db.commit()
        finally:
            await db.close()

    async def upsert_account(
        self,
        username: str,
        cookies: dict[str, str],
        headers: dict[str, str],
        twofa_secret: str | None = None,
    ) -> None:
        now = datetime.now(timezone.utc).isoformat(timespec="seconds")
        async with self._conn() as db:
            await db.execute(
                """
                INSERT INTO accounts (username, cookies_json, headers_json, twofa_secret, last_refresh, status)
                VALUES (?, ?, ?, ?, ?, 'active')
                ON CONFLICT(username) DO UPDATE SET
                    cookies_json = excluded.cookies_json,
                    headers_json = excluded.headers_json,
                    twofa_secret = COALESCE(excluded.twofa_secret, accounts.twofa_secret),
                    last_refresh = excluded.last_refresh,
                    status = 'active'
                """,
                (username, json.dumps(cookies), json.dumps(headers), twofa_secret, now),
            )

    async def get_account(self, username: str) -> Account | None:
        async with self._conn() as db:
            async with db.execute(
                "SELECT username, cookies_json, headers_json, twofa_secret, last_refresh, status "
                "FROM accounts WHERE username = ?",
                (username,),
            ) as cur:
                row = await cur.fetchone()
        if row is None:
            return None
        return Account(
            username=row["username"],
            cookies=json.loads(row["cookies_json"]),
            headers=json.loads(row["headers_json"]),
            twofa_secret=row["twofa_secret"],
            last_refresh=row["last_refresh"],
            status=row["status"],
        )

    async def list_accounts(self) -> list[Account]:
        async with self._conn() as db:
            async with db.execute(
                "SELECT username, cookies_json, headers_json, twofa_secret, last_refresh, status "
                "FROM accounts ORDER BY username"
            ) as cur:
                rows = await cur.fetchall()
        return [
            Account(
                username=r["username"],
                cookies=json.loads(r["cookies_json"]),
                headers=json.loads(r["headers_json"]),
                twofa_secret=r["twofa_secret"],
                last_refresh=r["last_refresh"],
                status=r["status"],
            )
            for r in rows
        ]

    async def delete_account(self, username: str) -> bool:
        async with self._conn() as db:
            cur = await db.execute("DELETE FROM accounts WHERE username = ?", (username,))
            return cur.rowcount > 0

    async def mark_expired(self, username: str) -> None:
        async with self._conn() as db:
            await db.execute(
                "UPDATE accounts SET status = 'expired' WHERE username = ?", (username,)
            )

    async def has_schema(self) -> bool:
        """判断 trading-engine 那边的 migration 是否已经跑过。"""
        async with self._conn() as db:
            async with db.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='accounts'"
            ) as cur:
                return await cur.fetchone() is not None
