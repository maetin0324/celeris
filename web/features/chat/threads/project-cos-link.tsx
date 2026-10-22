import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { Button } from "../../../components/ui/button";
import { openThread } from "../data/client";

/** ADR 2026-10-22-cos-fixed-threads D5: from a project to that project's CoS thread on the home. */
export function ProjectCosLink({ projectId, open = openThread }: { projectId: string; open?: typeof openThread }) {
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const go = async () => {
    setBusy(true);
    setFailed(false);
    try {
      const response = await open({ project_id: projectId });
      void navigate({ to: "/", search: { thread: response.thread.id } });
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <Button className="min-h-11" disabled={busy} onClick={() => void go()} data-testid="project-cos-link">
        この案件の CoS
      </Button>
      {failed && (
        <span role="alert" className="text-body text-destructive">
          CoS の会話を開けませんでした
        </span>
      )}
    </span>
  );
}
