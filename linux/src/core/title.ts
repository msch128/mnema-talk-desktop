/** The title of the host's windows and dialogs. */
export const TITLE = 'Mnema Desktop DEV'

/** The window's title: the front server's name before the product, or the product alone while none is in front. */
export function windowTitle(state: { active: string | null, instances: readonly { id: string, name: string }[] }): string {
  const front = state.instances.find((i) => i.id === state.active)
  return front ? `${front.name} \u2014 ${TITLE}` : TITLE
}
