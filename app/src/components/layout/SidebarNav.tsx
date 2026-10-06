import type { LucideIcon } from "lucide-react";
import { Button } from "@/components/ui/button";

export type NavItem = {
  label: string;
  icon: LucideIcon;
};

/**
 * Navigation placeholders for screens that do not exist yet. There is no
 * router: every item is unavailable. Items stay visible and focusable (so
 * keyboard and screen-reader users can discover them) but are marked
 * `aria-disabled`, labelled "Not available yet", and do nothing when
 * activated. Give an item a real action only when its screen exists.
 */
export function SidebarNav({ items }: { items: NavItem[] }) {
  return (
    <nav aria-label="Main">
      <ul className="flex flex-wrap gap-1 md:flex-col">
        {items.map(({ label, icon: Icon }) => (
          <li key={label}>
            <Button
              type="button"
              variant="ghost"
              aria-disabled="true"
              className="h-auto w-full justify-start gap-2.5 py-1.5 text-left aria-disabled:cursor-not-allowed aria-disabled:hover:bg-transparent"
            >
              <Icon aria-hidden="true" className="text-muted-foreground" />
              <span className="flex flex-col">
                <span className="text-sidebar-foreground/70">{label}</span>
                <span className="text-xs font-normal text-muted-foreground">Not available yet</span>
              </span>
            </Button>
          </li>
        ))}
      </ul>
    </nav>
  );
}
