import os
import sqlite3
from datetime import datetime
from pathlib import Path

from fastapi import FastAPI, HTTPException, Query


DATABASE = Path(os.environ.get("BENCHMARK_DATABASE", "../benchmark.sqlite")).resolve()
app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)


def connect() -> sqlite3.Connection:
    connection = sqlite3.connect(f"file:{DATABASE}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    return connection


def task_payload(row: sqlite3.Row) -> dict:
    return {
        "id": row["id"],
        "author": {"id": row["author_id"], "username": row["username"]},
        "name": row["name"],
        "created_at": time_payload(row["created_at"]),
        "updated_at": time_payload(row["updated_at"]),
    }


def time_payload(value: str) -> list[int]:
    timestamp = datetime.fromisoformat(value.replace("Z", "+00:00"))
    return [
        timestamp.year,
        timestamp.timetuple().tm_yday,
        timestamp.hour,
        timestamp.minute,
        timestamp.second,
        timestamp.microsecond * 1000,
        0,
        0,
        0,
    ]


@app.get("/api/tasks/")
def list_tasks(
    limit: int = Query(default=50, ge=0, le=100),
    offset: int = Query(default=0, ge=0),
    ordering: str = "id",
    name__contains: str | None = None,
):
    if ordering not in {"id", "-id", "name", "-name", "created_at", "-created_at", "updated_at", "-updated_at"}:
        raise HTTPException(status_code=400, detail=f"unknown ordering field: {ordering.lstrip('-')}")
    order = ordering.lstrip("-")
    direction = "DESC" if ordering.startswith("-") else "ASC"
    where = ""
    values: list[object] = []
    if name__contains is not None:
        where = " WHERE t.name LIKE ? ESCAPE '\\'"
        values.append(f"%{name__contains}%")
    with connect() as connection:
        count = connection.execute(
            f"SELECT COUNT(*) FROM tasks_task t{where}", values
        ).fetchone()[0]
        rows = connection.execute(
            f"""SELECT t.id, t.author_id, u.username, t.name, t.created_at, t.updated_at
                FROM tasks_task t JOIN auth_users u ON u.id = t.author_id
                {where} ORDER BY t.{order} {direction} LIMIT ? OFFSET ?""",
            [*values, limit, offset],
        ).fetchall()
    return {"count": count, "results": [task_payload(row) for row in rows]}


@app.get("/api/tasks/{task_id}/")
def retrieve_task(task_id: int):
    with connect() as connection:
        row = connection.execute(
            """SELECT t.id, t.author_id, u.username, t.name, t.created_at, t.updated_at
               FROM tasks_task t JOIN auth_users u ON u.id = t.author_id WHERE t.id = ?""",
            [task_id],
        ).fetchone()
    if row is None:
        raise HTTPException(status_code=404, detail="Not Found")
    return task_payload(row)
