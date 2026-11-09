from django.db import models


class User(models.Model):
    username = models.CharField(max_length=150)

    class Meta:
        managed = False
        db_table = "auth_users"


class Task(models.Model):
    author = models.ForeignKey(User, db_column="author_id", on_delete=models.DO_NOTHING)
    assignee = models.ForeignKey(User, db_column="assignee_id", on_delete=models.DO_NOTHING, null=True, related_name="assigned_tasks")
    name = models.TextField()
    status = models.TextField()
    created_at = models.DateTimeField()
    updated_at = models.DateTimeField()

    class Meta:
        managed = False
        db_table = "tasks_task"
