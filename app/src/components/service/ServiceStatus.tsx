import { CircleAlert, CircleCheck, CircleStop, LoaderCircle, RefreshCw } from "lucide-react";
import { cn } from "cn";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import type { LocalService } from "@/service/useLocalService";
import type { SupervisorState } from "@/service/types";
import { describeServiceStatus, type ServiceTone } from "./serviceStatusText";

const TONE_ICON = {
  pending: LoaderCircle,
  ready: CircleCheck,
  unavailable: CircleAlert,
  stopped: CircleStop,
} satisfies Record<ServiceTone, unknown>;

const TONE_COLOR: Record<ServiceTone, string> = {
  pending: "text-muted-foreground",
  ready: "text-emerald-700 dark:text-emerald-400",
  unavailable: "text-destructive",
  stopped: "text-muted-foreground",
};

function StatusIcon({ tone, className }: { tone: ServiceTone; className?: string }) {
  const Icon = TONE_ICON[tone];
  return (
    <Icon
      aria-hidden="true"
      className={cn(
        "size-4 shrink-0",
        TONE_COLOR[tone],
        tone === "pending" && "motion-safe:animate-spin",
        className,
      )}
    />
  );
}

/** Compact one-line status, e.g. for the sidebar. Not a live region. */
export function ServiceStatusIndicator({ status }: { status: SupervisorState }) {
  const text = describeServiceStatus(status);
  return (
    <p className="flex items-center gap-2 text-xs text-muted-foreground">
      <StatusIcon tone={text.tone} className="size-3.5" />
      <span>{text.label}</span>
    </p>
  );
}

/**
 * Full status card: what the service is doing, and -- while ready -- the
 * result of the authenticated health check. All values shown here are
 * rendered as plain text.
 */
export function ServiceStatusPanel({ service }: { service: LocalService }) {
  const { status, health, healthError, healthPending, recheckHealth } = service;
  const text = describeServiceStatus(status);
  const ready = status.state === "ready";

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <StatusIcon tone={text.tone} />
          {/* The one live region for service status changes. */}
          <span role="status" aria-live="polite">
            {text.label}
          </span>
        </CardTitle>
        <CardDescription>{text.description}</CardDescription>
        {ready && (
          <CardAction>
            {/* `aria-disabled` rather than `disabled` while a check is
                pending: disabling the focused button would drop keyboard
                focus to the document body. */}
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                if (!healthPending) recheckHealth();
              }}
              aria-disabled={healthPending || undefined}
              aria-label="Check local service health again"
              className="aria-disabled:cursor-not-allowed aria-disabled:opacity-50"
            >
              <RefreshCw
                aria-hidden="true"
                data-icon="inline-start"
                className={cn(healthPending && "motion-safe:animate-spin")}
              />
              Check again
            </Button>
          </CardAction>
        )}
      </CardHeader>

      {(ready || text.technicalReason) && (
        <CardContent>
          {ready && <HealthResult health={health} error={healthError} pending={healthPending} />}
          {text.technicalReason && (
            <p className="text-xs text-muted-foreground">
              Technical detail: <code className="font-mono">{text.technicalReason}</code>
            </p>
          )}
        </CardContent>
      )}
    </Card>
  );
}

function HealthResult({
  health,
  error,
  pending,
}: {
  health: LocalService["health"];
  error: string | null;
  pending: boolean;
}) {
  if (error) {
    return (
      <div className="text-sm">
        <p className="font-medium text-destructive">The health check failed.</p>
        <p className="mt-1 text-xs text-muted-foreground">
          Technical detail: <code className="font-mono">{error}</code>
        </p>
      </div>
    );
  }

  if (!health) {
    return <p className="text-sm text-muted-foreground">{pending ? "Checking service health…" : "No health result yet."}</p>;
  }

  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 rounded-lg bg-muted/50 px-4 py-3 text-sm">
      <dt className="text-muted-foreground">Health check</dt>
      <dd className="font-medium">{health.status}</dd>
      <dt className="text-muted-foreground">Service</dt>
      <dd>{health.service}</dd>
      <dt className="text-muted-foreground">Version</dt>
      <dd>{health.version}</dd>
    </dl>
  );
}
