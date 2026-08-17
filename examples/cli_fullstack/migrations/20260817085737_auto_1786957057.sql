-- Create "auth_users" table
CREATE TABLE `auth_users` (`id` integer NULL, `username` text NOT NULL, `password_hash` text NOT NULL, `is_active` integer NOT NULL DEFAULT true, `is_staff` integer NOT NULL DEFAULT false, `is_admin` integer NOT NULL DEFAULT false, `is_superuser` integer NOT NULL DEFAULT false, PRIMARY KEY (`id`));
-- Create index "auth_users_username" to table: "auth_users"
CREATE UNIQUE INDEX `auth_users_username` ON `auth_users` (`username`);
-- Create "auth_tokens" table
CREATE TABLE `auth_tokens` (`id` integer NULL, `user_id` integer NOT NULL, `key_hash` text NOT NULL, PRIMARY KEY (`id`), CONSTRAINT `0` FOREIGN KEY (`user_id`) REFERENCES `auth_users` (`id`) ON UPDATE NO ACTION ON DELETE CASCADE);
-- Create index "auth_tokens_key_hash" to table: "auth_tokens"
CREATE UNIQUE INDEX `auth_tokens_key_hash` ON `auth_tokens` (`key_hash`);
-- Create index "auth_tokens_idx_0" to table: "auth_tokens"
CREATE INDEX `auth_tokens_idx_0` ON `auth_tokens` (`user_id`);
-- Create "auth_sessions" table
CREATE TABLE `auth_sessions` (`id` integer NULL, `user_id` integer NOT NULL, `key_hash` text NOT NULL, `csrf_hash` text NOT NULL, `data` text NOT NULL, `revision` integer NOT NULL, `expires_at` text NOT NULL, `created_at` text NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')), PRIMARY KEY (`id`), CONSTRAINT `0` FOREIGN KEY (`user_id`) REFERENCES `auth_users` (`id`) ON UPDATE NO ACTION ON DELETE CASCADE);
-- Create index "auth_sessions_key_hash" to table: "auth_sessions"
CREATE UNIQUE INDEX `auth_sessions_key_hash` ON `auth_sessions` (`key_hash`);
-- Create index "auth_sessions_idx_0" to table: "auth_sessions"
CREATE INDEX `auth_sessions_idx_0` ON `auth_sessions` (`user_id`);
-- Create "tasks_task" table
CREATE TABLE `tasks_task` (`id` integer NULL, `author_id` integer NOT NULL, `name` text NOT NULL, `created_at` text NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')), `updated_at` text NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')), PRIMARY KEY (`id`), CONSTRAINT `0` FOREIGN KEY (`author_id`) REFERENCES `auth_users` (`id`) ON UPDATE NO ACTION ON DELETE CASCADE);
-- Create index "tasks_task_idx_0" to table: "tasks_task"
CREATE INDEX `tasks_task_idx_0` ON `tasks_task` (`author_id`);
-- Create "notifications_notification" table
CREATE TABLE `notifications_notification` (`id` integer NULL, `message` text NOT NULL, `created_at` text NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')), PRIMARY KEY (`id`));
