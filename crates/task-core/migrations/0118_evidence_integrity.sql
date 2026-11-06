-- N5: 起票時の承認条件を同じ transaction に固定する。旧 task は補完しない。
CREATE TABLE task_roadmap_specs (
    task_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    version INTEGER NOT NULL
);
CREATE TRIGGER task_roadmap_specs_capture AFTER INSERT ON tasks
WHEN NEW.parent_id IS NULL AND NEW.project_id IS NOT NULL
BEGIN
    INSERT INTO task_roadmap_specs
    SELECT NEW.id, NEW.project_id, version
    FROM project_roadmap_versions WHERE project_id = NEW.project_id AND status = 'approved';
END;
CREATE TRIGGER task_roadmap_specs_no_update BEFORE UPDATE ON task_roadmap_specs
BEGIN SELECT RAISE(ABORT, 'task_roadmap_specs is append-only'); END;
CREATE TRIGGER task_roadmap_specs_no_delete BEFORE DELETE ON task_roadmap_specs
BEGIN SELECT RAISE(ABORT, 'task_roadmap_specs is append-only'); END;
CREATE TRIGGER script_compare_log_keep_max BEFORE DELETE ON script_compare_log
WHEN OLD.id = (SELECT MAX(id) FROM script_compare_log)
BEGIN SELECT RAISE(ABORT, 'retention must preserve maximum id'); END;
CREATE TRIGGER project_wakes_keep_max BEFORE DELETE ON project_wakes
WHEN OLD.id = (SELECT MAX(id) FROM project_wakes)
BEGIN SELECT RAISE(ABORT, 'retention must preserve maximum id'); END;
CREATE TRIGGER project_controller_decisions_keep_max BEFORE DELETE ON project_controller_decisions
WHEN OLD.id = (SELECT MAX(id) FROM project_controller_decisions)
BEGIN SELECT RAISE(ABORT, 'retention must preserve maximum id'); END;
CREATE INDEX task_roadmap_specs_project ON task_roadmap_specs(project_id, version);
