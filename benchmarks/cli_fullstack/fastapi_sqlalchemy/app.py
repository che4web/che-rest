import os
from datetime import datetime
from pathlib import Path
from typing import Annotated

from fastapi import Depends, FastAPI, HTTPException, Query
from sqlalchemy import ForeignKey, String, create_engine, func, select
from sqlalchemy.orm import DeclarativeBase, Mapped, Session, mapped_column, relationship


DATABASE = Path(os.environ.get("BENCHMARK_DATABASE", "../benchmark.sqlite")).resolve()
engine = create_engine(
    f"sqlite:///{DATABASE}",
    connect_args={"check_same_thread": False, "uri": False},
    pool_size=4,
    max_overflow=0,
)
app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)


class Base(DeclarativeBase):
    pass


class User(Base):
    __tablename__ = "auth_users"

    id: Mapped[int] = mapped_column(primary_key=True)
    username: Mapped[str] = mapped_column(String)


class Task(Base):
    __tablename__ = "tasks_task"

    id: Mapped[int] = mapped_column(primary_key=True)
    author_id: Mapped[int] = mapped_column(ForeignKey("auth_users.id"))
    name: Mapped[str] = mapped_column(String)
    created_at: Mapped[datetime]
    updated_at: Mapped[datetime]
    author: Mapped[User] = relationship()


def session() -> Session:
    with Session(engine) as database:
        yield database


Database = Annotated[Session, Depends(session)]


def time_payload(value: datetime) -> list[int]:
    return [
        value.year,
        value.timetuple().tm_yday,
        value.hour,
        value.minute,
        value.second,
        value.microsecond * 1000,
        0,
        0,
        0,
    ]


def task_payload(task: Task) -> dict:
    return {
        "id": task.id,
        "author": {"id": task.author.id, "username": task.author.username},
        "name": task.name,
        "created_at": time_payload(task.created_at),
        "updated_at": time_payload(task.updated_at),
    }


ORDERING = {
    "id": Task.id,
    "name": Task.name,
    "created_at": Task.created_at,
    "updated_at": Task.updated_at,
}


@app.get("/api/tasks/")
def list_tasks(
    database: Database,
    limit: int = Query(default=50, ge=0, le=100),
    offset: int = Query(default=0, ge=0),
    ordering: str = "id",
    name__contains: str | None = None,
):
    field_name = ordering.lstrip("-")
    field = ORDERING.get(field_name)
    if field is None:
        raise HTTPException(status_code=400, detail=f"unknown ordering field: {field_name}")
    condition = Task.name.contains(name__contains) if name__contains is not None else None
    count_query = select(func.count()).select_from(Task)
    task_query = select(Task).join(Task.author)
    if condition is not None:
        count_query = count_query.where(condition)
        task_query = task_query.where(condition)
    task_query = task_query.order_by(field.desc() if ordering.startswith("-") else field.asc())
    task_query = task_query.limit(limit).offset(offset)
    count = database.scalar(count_query) or 0
    tasks = database.scalars(task_query).all()
    return {"count": count, "results": [task_payload(task) for task in tasks]}


@app.get("/api/tasks/{task_id}/")
def retrieve_task(task_id: int, database: Database):
    task = database.scalar(select(Task).join(Task.author).where(Task.id == task_id))
    if task is None:
        raise HTTPException(status_code=404, detail="Not Found")
    return task_payload(task)
