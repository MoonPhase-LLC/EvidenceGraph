import { ServiceStatusPanel } from "@/components/service/ServiceStatus";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import type { LocalService } from "@/service/useLocalService";

/**
 * The only screen in this build. It states plainly what is not available
 * yet and shows the local service's status; it shows no sample or
 * invented assessment data.
 */
export function HomeScreen({ service }: { service: LocalService }) {
  return (
    <div className="flex flex-col gap-10">
      <header className="flex flex-col gap-2">
        <h1 className="text-2xl font-semibold tracking-tight">Welcome to EvidenceGraph</h1>
        <p className="max-w-prose text-muted-foreground">
          EvidenceGraph is a local-first tool for analyzing cybersecurity compliance evidence
          against framework controls.
        </p>
      </header>

      <section aria-labelledby="assessments-heading" className="flex flex-col gap-3">
        <h2 id="assessments-heading" className="text-lg font-semibold tracking-tight">
          Assessments
        </h2>
        <Card>
          <CardHeader>
            <CardTitle>Assessment features are not available yet</CardTitle>
            <CardDescription className="max-w-prose">
              Creating assessments, uploading evidence, and reviewing suggested control mappings
              will be added in later releases. Nothing can be created or uploaded in this
              version.
            </CardDescription>
          </CardHeader>
        </Card>
      </section>

      <section aria-labelledby="service-heading" className="flex flex-col gap-3">
        <h2 id="service-heading" className="text-lg font-semibold tracking-tight">
          Local service
        </h2>
        <ServiceStatusPanel service={service} />
      </section>
    </div>
  );
}
