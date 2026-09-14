import norixorIcon from "./norixor-icon.png";
import dockerIcon from "./docker-icon.png";
import systemdIcon from "./systemd-icon.png";
import diskIcon from "./disk-icon.png";
import networkIcon from "./network-icon.png";
import processesIcon from "./processes-icon.png";
import logsIcon from "./logs-icon.png";
import cronIcon from "./cron-icon.png";
import tunnelsIcon from "./tunnels-icon.png";
import nginxIcon from "./nginx-icon.png";
import tasksIcon from "./tasks-icon.png";

// Use this built-in fallback while marketplace network icons are unavailable; Core manages network and disk caching.
export const officialPluginIcons: Readonly<Record<string, string>> = {
  "org.norixor": norixorIcon,
  "org.norishell.docker": dockerIcon,
  "org.norishell.systemd": systemdIcon,
  "org.norishell.disk": diskIcon,
  "org.norishell.network": networkIcon,
  "org.norishell.processes": processesIcon,
  "org.norishell.logs": logsIcon,
  "org.norishell.cron": cronIcon,
  "org.norishell.tunnels": tunnelsIcon,
  "org.norishell.nginx": nginxIcon,
  "org.norishell.tasks": tasksIcon,
};
