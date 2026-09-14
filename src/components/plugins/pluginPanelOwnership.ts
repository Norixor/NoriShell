import { shallowRef } from "vue";

// Host-owned tool and floating panels share one expanded surface in this WebView.
export const activePluginPanel = shallowRef<string | null>(null);
