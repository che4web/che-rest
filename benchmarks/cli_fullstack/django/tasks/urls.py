from django.urls import path

from .views import list_tasks, retrieve_task


urlpatterns = [
    path("api/tasks/", list_tasks),
    path("api/tasks/<int:task_id>/", retrieve_task),
]
