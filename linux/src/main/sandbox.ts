/**
 * The renderer sandbox of Chromium is part of the security model: the signed
 * app is trusted code, but a bug in how it renders what other members send
 * must not become code execution on the machine. A start without it is refused
 * unless the user says so.
 *
 * `hasSwitch` is `app.commandLine.hasSwitch`: Chromium takes its switches off
 * `process.argv`, so only its own command line shows them.
 *
 * The AppImage launcher adds `--no-sandbox` by itself where the system forbids
 * unprivileged user namespaces (Ubuntu 24.04 and newer, by default). The .deb
 * installs an AppArmor profile that allows them for this app.
 */
export function sandboxDisabled(hasSwitch: (name: string) => boolean, env: NodeJS.ProcessEnv): boolean {
  return hasSwitch('no-sandbox') || hasSwitch('disable-setuid-sandbox') || (env.ELECTRON_DISABLE_SANDBOX !== undefined && env.ELECTRON_DISABLE_SANDBOX !== '')
}

export function sandboxAccepted(env: NodeJS.ProcessEnv): boolean {
  return env.MNEMA_DESKTOP_ALLOW_NO_SANDBOX === '1'
}

export const SANDBOX_MESSAGE = [
  'The browser sandbox is turned off, so Mnema Desktop does not start.',
  '',
  'This usually means the system does not allow unprivileged user namespaces (Ubuntu 24.04 and newer restrict them with AppArmor), and the AppImage launcher then drops the sandbox.',
  '',
  'Install the .deb package, which brings an AppArmor profile for this app, or allow user namespaces, for example with:',
  '  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0',
  '',
  'To start without the sandbox anyway, set MNEMA_DESKTOP_ALLOW_NO_SANDBOX=1.',
].join('\n')

/**
 * A packaged app does not open a remote debugging port: any program of the
 * user could attach to it and read what the page holds, such as the keys of the
 * end-to-end encryption. Development runs and the end-to-end test opt in.
 */
export function debuggingRequested(hasSwitch: (name: string) => boolean): boolean {
  return ['remote-debugging-port', 'remote-debugging-pipe', 'remote-debugging-address', 'remote-allow-origins', 'inspect', 'inspect-brk', 'inspect-port'].some(hasSwitch)
}

export function debuggingAccepted(env: NodeJS.ProcessEnv): boolean {
  return env.MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING === '1'
}
