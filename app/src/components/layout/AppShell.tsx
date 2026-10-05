import type { ReactNode } from "react";
import { Waypoints } from "lucide-react";

/**
 * The desktop window frame: a sidebar (brand, navigation, footer slot) and
 * a scrolling main region. Below the `md` breakpoint the sidebar becomes a
 * top bar so narrow windows keep the full content width.
 */
export function AppShell({
  navigation,
  sidebarFooter,
  children,
}: {
  navigation: ReactNode;
  sidebarFooter?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex min-h-screen flex-col bg-background text-foreground md:h-screen md:flex-row">
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-background focus:px-3 focus:py-2 focus:text-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-solid focus-visible:outline-foreground"
      >
        Skip to main content
      </a>

      <aside className="flex shrink-0 flex-wrap items-center gap-x-6 gap-y-3 border-b border-sidebar-border bg-sidebar px-4 py-3 text-sidebar-foreground md:w-60 md:flex-col md:flex-nowrap md:items-stretch md:gap-6 md:border-r md:border-b-0 md:px-3 md:py-5">
        <div className="flex items-center gap-2 px-2 text-base font-semibold tracking-tight">
          <Waypoints aria-hidden="true" className="size-5 text-sidebar-primary" />
          <span>EvidenceGraph</span>
        </div>
        {navigation}
        {sidebarFooter && (
          <div className="ml-auto px-2 md:mt-auto md:ml-0">{sidebarFooter}</div>
        )}
      </aside>

      <main id="main-content" tabIndex={-1} className="min-w-0 flex-1 outline-none md:overflow-y-auto">
        <div className="mx-auto w-full max-w-3xl px-5 py-8 md:px-10 md:py-12">{children}</div>
      </main>
    </div>
  );
}
