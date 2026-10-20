// ============================================
// Embedded OpenCode Server (Android)
//
// The Android build runs the opencode v2 server inside the app: a `node`
// runtime shipped in the APK executes the server bundle that is installed
// from the APK assets into the app's private storage.
//
// The desktop build spawns the `opencode` binary and the web build talks to
// a remote server; on Android the app *is* the server.
// ============================================

import { invoke } from '@tauri-apps/api/core'

export interface EmbeddedServerInfo {
  /** Path of the bundled node runtime. */
  runtime: string | null
  /** Directory the server bundle was installed to. */
  bundleDir: string | null
  /** Entry point of the server bundle (opencode.mjs). */
  entry: string | null
  /** Workspace directory the server runs in. */
  workspace: string | null
  /** Whether a server process is running. */
  running: boolean
  /** URL the running server listens on. */
  url: string | null
  error?: string | null
}

export interface EmbeddedServerResult {
  started: boolean
  url: string | null
  serverPassword: string | null
  pid: number | null
  workspace: string | null
  error: string | null
}

export interface WorkspaceEntry {
  name: string
  path: string
  isDir: boolean
  writable: boolean
}

// ============================================
// Native Android bridge (Kotlin)
// ============================================

export interface AndroidNativeBridge {
  /** Copies an APK asset tree into the app's private storage. */
  installAssets?: (prefix: string) => string
  /** nativeLibraryDir — the only directory an app may exec() from. */
  nativeLibraryDir?: () => string
  /** Whether the app may read/write shared storage. */
  hasStoragePermission?: () => boolean
  /** Opens the system screen that grants all-files access. */
  requestStoragePermission?: () => void
  vibrate?: (ms: number) => void
}

export function androidBridge(): AndroidNativeBridge | undefined {
  if (typeof window === 'undefined') return undefined
  return (window as unknown as { __opencode_android?: AndroidNativeBridge }).__opencode_android
}

/**
 * The Kotlin bridge is attached to the WebView shortly after the activity is
 * created, so it is not there on the very first tick of the app.
 */
export async function waitForAndroidBridge(timeoutMs = 8000): Promise<AndroidNativeBridge | undefined> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const bridge = androidBridge()
    if (bridge?.installAssets) return bridge
    if (Date.now() > deadline) return bridge
    await new Promise(resolve => setTimeout(resolve, 150))
  }
}

/**
 * Expands the bundled server (APK assets) into the app's private storage.
 * Returns the destination directory, or undefined when the assets are not
 * available (desktop / web).
 */
export async function installServerAssets(prefix = 'opencode'): Promise<string | undefined> {
  const bridge = await waitForAndroidBridge()
  if (!bridge?.installAssets) return undefined
  try {
    const dir = bridge.installAssets(prefix)
    return dir || undefined
  } catch {
    return undefined
  }
}

/** True when the app can read and write shared storage (/sdcard). */
export function hasStorageAccess(): boolean {
  try {
    return androidBridge()?.hasStoragePermission?.() ?? false
  } catch {
    return false
  }
}

/**
 * Opens the system screen where the user grants all-files access. Android
 * does not return a result to the app: the caller re-checks
 * `hasStorageAccess()` when the app comes back to the foreground.
 */
export function requestStorageAccess(): void {
  try {
    androidBridge()?.requestStoragePermission?.()
  } catch {
    /* the bridge is only present on Android */
  }
}

// ============================================
// Commands
// ============================================

export async function getEmbeddedServerInfo(): Promise<EmbeddedServerInfo | undefined> {
  try {
    return await invoke<EmbeddedServerInfo>('embedded_server_info')
  } catch {
    return undefined
  }
}

export async function startEmbeddedServer(options: {
  workspace?: string | null
  envVars?: Record<string, string>
  extraArgs?: string[]
}): Promise<EmbeddedServerResult> {
  return invoke<EmbeddedServerResult>('start_embedded_server', {
    workspace: options.workspace ?? null,
    envVars: options.envVars ?? {},
    extraArgs: options.extraArgs ?? [],
  })
}

export async function stopEmbeddedServer(): Promise<boolean> {
  try {
    return await invoke<boolean>('stop_embedded_server')
  } catch {
    return false
  }
}

export async function workspaceCandidates(): Promise<string[]> {
  try {
    return await invoke<string[]>('workspace_candidates')
  } catch {
    return []
  }
}

export async function listWorkspaceDirs(path: string): Promise<WorkspaceEntry[]> {
  return invoke<WorkspaceEntry[]>('list_workspace_dirs', { path })
}
