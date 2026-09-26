import { inject, onActivated, onBeforeUnmount, onDeactivated, type InjectionKey } from "vue";
import { useRoute } from "vue-router";

type RouteRevealRegistration = (path: string) => () => void;

export const routeRevealKey: InjectionKey<RouteRevealRegistration> = Symbol("route-reveal");

export function useRouteReveal(): () => void {
  const register = inject(routeRevealKey, null);
  const path = useRoute().path;
  let reveal = register?.(path);
  let active = true;
  onActivated(() => { active = true; reveal = register?.(path); });
  onDeactivated(() => { active = false; });
  onBeforeUnmount(() => { active = false; });
  return () => { if (active) reveal?.(); };
}
