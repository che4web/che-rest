import os
import sqlite3
from contextlib import closing
from datetime import datetime, timezone
from pathlib import Path

from fastapi import FastAPI, HTTPException, Query


DATABASE = Path(os.environ.get("BENCHMARK_DATABASE", "../benchmark.sqlite")).resolve()
POSTGRES_URL = os.environ.get("BENCHMARK_POSTGRES_URL")
if POSTGRES_URL:
    import psycopg2
    from psycopg2.extras import DictCursor
app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)


def connect():
    if POSTGRES_URL:
        connection = psycopg2.connect(POSTGRES_URL, cursor_factory=DictCursor)
        connection.autocommit = True
        return connection
    connection = sqlite3.connect(f"file:{DATABASE}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    return connection


def task_payload(row: sqlite3.Row) -> dict:
    return {
        "id": row["id"],
        "author": {"id": row["author_id"], "username": row["username"]},
        "assignee_id": row["assignee_id"],
        "name": row["name"],
        "status": row["status"],
        "created_at": time_payload(row["created_at"]),
        "updated_at": time_payload(row["updated_at"]),
    }


def time_payload(value: str) -> str:
    timestamp = value if isinstance(value, datetime) else datetime.fromisoformat(value.replace("Z", "+00:00"))
    return timestamp.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


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
    with closing(connect()) as connection, closing(connection.cursor()) as cursor:
        placeholder = "%s" if POSTGRES_URL else "?"
        # SQL identifiers come only from the ordering allowlist above.
        sql_where = where.replace("?", placeholder)
        cursor.execute(
            f"SELECT COUNT(*) FROM tasks_task t{sql_where}", values
        )
        count = cursor.fetchone()[0]
        query = (
            f"""SELECT t.id, t.author_id, u.username, t.assignee_id, t.name, t.status, t.created_at, t.updated_at
                FROM tasks_task t JOIN auth_users u ON u.id = t.author_id
                {sql_where} ORDER BY t.{order} {direction} LIMIT {placeholder} OFFSET {placeholder}"""
        )
        cursor.execute(query, [*values, limit, offset])
        rows = cursor.fetchall()
    return {"count": count, "results": [task_payload(row) for row in rows]}


@app.get("/api/tasks/{task_id}/")
def retrieve_task(task_id: int):
    with closing(connect()) as connection, closing(connection.cursor()) as cursor:
        query = (
            """SELECT t.id, t.author_id, u.username, t.assignee_id, t.name, t.status, t.created_at, t.updated_at
               FROM tasks_task t JOIN auth_users u ON u.id = t.author_id WHERE t.id = """
            + ("%s" if POSTGRES_URL else "?")
        )
        cursor.execute(query, [task_id])
        row = cursor.fetchone()
    if row is None:
        raise HTTPException(status_code=404, detail="Not Found")
    return task_payload(row)
