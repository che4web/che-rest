from django.http import JsonResponse
from django.views.decorators.http import require_GET

from .models import Task


ORDERING = {"id": "id", "name": "name", "created_at": "created_at", "updated_at": "updated_at"}


def time_payload(value):
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


def task_payload(task):
    return {
        "id": task.id,
        "author": {"id": task.author.id, "username": task.author.username},
        "name": task.name,
        "created_at": time_payload(task.created_at),
        "updated_at": time_payload(task.updated_at),
    }


@require_GET
def list_tasks(request):
    limit = min(int(request.GET.get("limit", 50)), 100)
    offset = max(int(request.GET.get("offset", 0)), 0)
    ordering = request.GET.get("ordering", "id")
    field_name = ordering.lstrip("-")
    field = ORDERING.get(field_name)
    if field is None:
        return JsonResponse({"detail": f"unknown ordering field: {field_name}"}, status=400)
    tasks = Task.objects.select_related("author")
    name = request.GET.get("name__contains")
    if name is not None:
        tasks = tasks.filter(name__contains=name)
    count = tasks.count()
    tasks = tasks.order_by(f"-{field}" if ordering.startswith("-") else field)[offset : offset + limit]
    return JsonResponse({"count": count, "results": [task_payload(task) for task in tasks]})


@require_GET
def retrieve_task(request, task_id):
    try:
        task = Task.objects.select_related("author").get(id=task_id)
    except Task.DoesNotExist:
        return JsonResponse({"detail": "Not Found"}, status=404)
    return JsonResponse(task_payload(task))
