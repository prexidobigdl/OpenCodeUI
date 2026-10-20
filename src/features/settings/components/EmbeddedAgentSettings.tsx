// ============================================
// Embedded Agent Settings (Android)
//
// The Android build runs the opencode server inside the app. This panel
// configures that server: the workspace the agent works in, the provider
// credentials it receives, and the server lifecycle.
// ============================================

import { useCallback, useEffect, useState } from 'react'
import { Button } from '../../../components/ui/Button'
import { SpinnerIcon, StopIcon, FolderIcon } from '../../../components/Icons'
import {
  getEmbeddedServerInfo,
  hasStorageAccess,
  installServerAssets,
  listWorkspaceDirs,
  requestStorageAccess,
  startEmbeddedServer,
  stopEmbeddedServer,
  workspaceCandidates,
  type EmbeddedServerInfo,
  type WorkspaceEntry,
} from '../../../api/embedded'
import { EMBEDDED_PROVIDERS, embeddedStore, useEmbeddedServerStore } from '../../../store/embeddedStore'
import { applyLocalServiceUrl } from '../../../utils/localServiceUrl'
import { apiErrorHandler } from '../../../utils'
import { SettingField, SettingRow, SettingsSection, Toggle, settingsFieldClass } from './SettingsUI'

export function EmbeddedAgentSettings() {
  const { workspace, envVars, autoStart } = useEmbeddedServerStore()
  const [info, setInfo] = useState<EmbeddedServerInfo | undefined>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [browsing, setBrowsing] = useState(false)
  const [browsePath, setBrowsePath] = useState('')
  const [entries, setEntries] = useState<WorkspaceEntry[]>([])
  const [storageGranted, setStorageGranted] = useState(() => hasStorageAccess())

  // The grant happens in a system screen, so re-check when the app returns.
  useEffect(() => {
    const update = () => setStorageGranted(hasStorageAccess())
    document.addEventListener('visibilitychange', update)
    window.addEventListener('focus', update)
    return () => {
      document.removeEventListener('visibilitychange', update)
      window.removeEventListener('focus', update)
    }
  }, [])

  const refresh = useCallback(async () => {
    const next = await getEmbeddedServerInfo()
    setInfo(next)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const handleStart = async () => {
    setBusy(true)
    setError('')
    if (workspace && !hasStorageAccess()) {
      setStorageGranted(false)
      setError('Storage access is required for this workspace — grant it below, then start again.')
      setBusy(false)
      requestStorageAccess()
      return
    }
    try {
      await installServerAssets('opencode')
      const result = await startEmbeddedServer({
        workspace: workspace || null,
        envVars: embeddedStore.activeEnvVars,
      })
      applyLocalServiceUrl(result.url, result.serverPassword)
      if (result.error) setError(result.error)
    } catch (err) {
      apiErrorHandler('start embedded opencode server', err)
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
      void refresh()
    }
  }

  const handleStop = async () => {
    setBusy(true)
    setError('')
    try {
      await stopEmbeddedServer()
    } catch (err) {
      apiErrorHandler('stop embedded opencode server', err)
    } finally {
      setBusy(false)
      void refresh()
    }
  }

  const handleBrowse = async (path?: string) => {
    setBusy(true)
    setError('')
    try {
      const target = path ?? browsePath ?? (await workspaceCandidates())[0] ?? '/sdcard'
      setBrowsePath(target)
      const list = await listWorkspaceDirs(target)
      setEntries(list.filter(entry => entry.isDir))
      setBrowsing(true)
    } catch (err) {
      apiErrorHandler('list workspace directories', err)
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleSelectWorkspace = async (path: string) => {
    embeddedStore.setWorkspace(path)
    setBrowsing(false)
    // A new workspace needs a new server process (the working directory is
    // fixed at spawn time).
    try {
      await stopEmbeddedServer()
    } catch {
      /* ignore */
    }
    await handleStart()
  }

  const statusText = info?.running ? `Running · ${info.url ?? ''}` : 'Stopped'

  return (
    <div className="flex flex-col gap-6">
      <SettingsSection title="On-device server" description="OpenCode runs inside the app on this device.">
        <SettingRow label="Status" description={statusText}>
          {info?.running ? (
            <Button variant="secondary" size="sm" onClick={() => void handleStop()} disabled={busy}>
              <StopIcon size={14} />
              Stop
            </Button>
          ) : (
            <Button variant="primary" size="sm" onClick={() => void handleStart()} disabled={busy}>
              {busy ? <SpinnerIcon size={14} /> : null}
              Start
            </Button>
          )}
        </SettingRow>

        <SettingField label="Runtime" description={info?.runtime ?? 'node runtime not found in the APK'}>
          <div className="text-[length:var(--fs-xs)] font-mono text-text-400 break-all">
            {info?.runtime ?? '—'}
          </div>
        </SettingField>

        <SettingField label="Server bundle" description={info?.entry ?? 'assets not installed yet'}>
          <div className="text-[length:var(--fs-xs)] font-mono text-text-400 break-all">{info?.entry ?? '—'}</div>
        </SettingField>

        <SettingRow label="Start with the app" description="Launch the server automatically when OpenCode opens.">
          <Toggle checked={autoStart} onChange={value => embeddedStore.setAutoStart(value)} />
        </SettingRow>

        {error ? <div className="text-[length:var(--fs-xs)] text-red-500 break-all">{error}</div> : null}
      </SettingsSection>

      <SettingsSection
        title="Workspace"
        description="The directory the agent reads and writes. Needs storage access on the device."
      >
        {storageGranted ? (
          <div className="text-[length:var(--fs-xs)] text-text-400">
            Storage access granted · the agent can work anywhere on the device
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-3 rounded-lg border border-yellow-500/40 bg-yellow-500/10 px-3 py-2">
            <div className="text-[length:var(--fs-xs)] text-text-200">
              Storage access is not granted. Without it the agent can only work inside the app's private
              storage. Grant “All files access” so it can work in /sdcard.
            </div>
            <Button variant="secondary" size="sm" className="ml-auto" onClick={() => requestStorageAccess()}>
              Grant
            </Button>
          </div>
        )}

        <SettingField label="Current workspace" description={workspace || 'No workspace selected'}>
          <div className="flex items-center gap-2">
            <input
              className={settingsFieldClass}
              value={workspace}
              onChange={event => embeddedStore.setWorkspace(event.target.value)}
              placeholder="/sdcard/Documents/project"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
            />
            <Button variant="secondary" size="sm" onClick={() => void handleBrowse()} disabled={busy}>
              <FolderIcon size={14} />
              Browse
            </Button>
          </div>
        </SettingField>

        {browsing ? (
          <div className="rounded-lg border border-line-200 overflow-hidden">
            <div className="flex items-center gap-2 px-3 py-2 bg-bg-100 text-[length:var(--fs-xs)] font-mono text-text-400">
              <span className="truncate">{browsePath}</span>
              <button
                type="button"
                className="ml-auto text-accent-main-100 shrink-0"
                onClick={() => {
                  const parent = browsePath.split('/').slice(0, -1).join('/') || '/'
                  void handleBrowse(parent)
                }}
              >
                Up
              </button>
              <button type="button" className="text-accent-main-100 shrink-0" onClick={() => setBrowsing(false)}>
                Close
              </button>
            </div>
            <div className="max-h-56 overflow-y-auto custom-scrollbar">
              {entries.length === 0 ? (
                <div className="px-3 py-2 text-[length:var(--fs-xs)] text-text-400">No subdirectories</div>
              ) : (
                entries.map(entry => (
                  <button
                    key={entry.path}
                    type="button"
                    className="w-full flex items-center gap-2 px-3 py-2 text-left text-[length:var(--fs-sm)] text-text-200 hover:bg-bg-200"
                    onClick={() => void handleBrowse(entry.path)}
                    onDoubleClick={() => void handleSelectWorkspace(entry.path)}
                  >
                    <FolderIcon size={14} />
                    <span className="truncate">{entry.name}</span>
                  </button>
                ))
              )}
            </div>
            <div className="px-3 py-2 border-t border-line-200">
              <Button variant="primary" size="sm" onClick={() => void handleSelectWorkspace(browsePath)} disabled={busy}>
                Use this folder
              </Button>
            </div>
          </div>
        ) : null}
      </SettingsSection>

      <SettingsSection
        title="Provider credentials"
        description="Keys are handed to the on-device server as environment variables and stored on the device only."
      >
        {EMBEDDED_PROVIDERS.map(provider => (
          <SettingField key={provider.key} label={provider.label} description={provider.key}>
            <input
              className={settingsFieldClass}
              type="password"
              value={envVars[provider.key] ?? ''}
              onChange={event => embeddedStore.setEnvVar(provider.key, event.target.value)}
              placeholder={provider.key}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
            />
          </SettingField>
        ))}
      </SettingsSection>
    </div>
  )
}
