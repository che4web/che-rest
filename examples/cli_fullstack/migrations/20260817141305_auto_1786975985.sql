-- Disable the enforcement of foreign-keys constraints
PRAGMA foreign_keys = off;
-- Create "new_tasks_task" table
CREATE TABLE `new_tasks_task` (`id` integer NULL, `author_id` integer NOT NULL, `assignee_id` integer NULL, `name` text NOT NULL, `status` text NOT NULL, `created_at` text NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')), `updated_at` text NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')), PRIMARY KEY (`id`), CONSTRAINT `0` FOREIGN KEY (`assignee_id`) REFERENCES `auth_users` (`id`) ON UPDATE NO ACTION ON DELETE SET NULL, CONSTRAINT `1` FOREIGN KEY (`author_id`) REFERENCES `auth_users` (`id`) ON UPDATE NO ACTION ON DELETE CASCADE, CHECK (status IN ('draft', 'in_progress', 'done')));
-- Copy rows from old table "tasks_task" to new temporary table "new_tasks_task"
INSERT INTO `new_tasks_task` (`id`, `author_id`, `name`, `status`, `created_at`, `updated_at`) SELECT `id`, `author_id`, `name`, `status`, `created_at`, `updated_at` FROM `tasks_task`;
-- Drop "tasks_task" table after copying rows
DROP TABLE `tasks_task`;
-- Rename temporary table "new_tasks_task" to "tasks_task"
ALTER TABLE `new_tasks_task` RENAME TO `tasks_task`;
-- Create index "tasks_task_idx_0" to table: "tasks_task"
CREATE INDEX `tasks_task_idx_0` ON `tasks_task` (`author_id`);
-- Create index "tasks_task_idx_1" to table: "tasks_task"
CREATE INDEX `tasks_task_idx_1` ON `tasks_task` (`assignee_id`);
-- Enable back the enforcement of foreign-keys constraints
PRAGMA foreign_keys = on;
