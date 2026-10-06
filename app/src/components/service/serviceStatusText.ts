import type { SupervisorState } from "@/service/types";

export type ServiceTone = "pending" | "ready" | "unavailable" | "stopped";

export type ServiceStatusText = {
  tone: ServiceTone;
  /** Short label, e.g. for the sidebar indicator. */
  label: string;
  /** One or two plain-language sentences. */
  description: string;
  /**
   * Fixed failure code from the supervisor (always one of its bounded,
   * non-secret reason strings), shown only as secondary detail.
   */
  technicalReason?: string;
};

const STARTING_STEPS: Record<
  "starting" | "awaiting_endpoint" | "verifying_identity" | "installing_credential",
  string
> = {
  starting: "Launching the local analysis service.",
  awaiting_endpoint: "Waiting for the service to report where it is listening.",
  verifying_identity: "Verifying the service's identity.",
  installing_credential: "Setting up a secure session with the service.",
};

/** The one place user-facing service status wording is defined. */
export function describeServiceStatus(status: SupervisorState): ServiceStatusText {
  switch (status.state) {
    case "ready":
      return {
        tone: "ready",
        label: "Service ready",
        description: "The local analysis service is running on this computer.",
      };
    case "failed":
      // `failed` covers both a startup that never completed and a service
      // that was running and was then lost -- the wording must fit both.
      return {
        tone: "unavailable",
        label: "Service unavailable",
        description:
          "The local analysis service is not running. It either could not start or stopped unexpectedly. Restart EvidenceGraph to try again.",
        technicalReason: status.reason,
      };
    case "stopped":
      return {
        tone: "stopped",
        label: "Service stopped",
        description: "The local analysis service has been shut down.",
      };
    default:
      return {
        tone: "pending",
        label: "Starting service",
        description: STARTING_STEPS[status.state],
      };
  }
}
