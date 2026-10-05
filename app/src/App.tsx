import { ClipboardList, Settings } from "lucide-react";
import { AppShell } from "@/components/layout/AppShell";
import { SidebarNav, type NavItem } from "@/components/layout/SidebarNav";
import { ServiceStatusIndicator } from "@/components/service/ServiceStatus";
import { HomeScreen } from "@/screens/HomeScreen";
import { useLocalService } from "@/service/useLocalService";

// Placeholders for future screens; none is navigable yet (see SidebarNav).
const NAV_ITEMS: NavItem[] = [
  { label: "Assessments", icon: ClipboardList },
  { label: "Settings", icon: Settings },
];

function App() {
  const service = useLocalService();

  return (
    <AppShell
      navigation={<SidebarNav items={NAV_ITEMS} />}
      sidebarFooter={<ServiceStatusIndicator status={service.status} />}
    >
      <HomeScreen service={service} />
    </AppShell>
  );
}

export default App;
