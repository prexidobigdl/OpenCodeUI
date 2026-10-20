// ============================================
// Embedded Server Store (Android)
//
// Holds the configuration of the opencode server that runs inside the app:
// the workspace the agent works in and the provider credentials handed to the
// server process as environment variables.
// ============================================

import { useSyncExternalStore } from 'react'

const STORAGE_KEY_WORKSPACE = 'opencode-embedded-workspace'
const STORAGE_KEY_ENV_VARS = 'opencode-embedded-env-vars'
const STORAGE_KEY_AUTO_START = 'opencode-embedded-auto-start'

/** Provider credentials the agent can use. */
export const EMBEDDED_PROVIDERS: { key: string; label: string }[] = [
  { key: 'ANTHROPIC_API_KEY', label: 'Anthropic' },
  { key: 'OPENAI_API_KEY', label: 'OpenAI' },
  { key: 'OPENROUTER_API_KEY', label: 'OpenRouter' },
  { key: 'GOOGLE_GENERATIVE_AI_API_KEY', label: 'Google Gemini' },
  { key: 'GROQ_API_KEY', label: 'Groq' },
  { key: 'XAI_API_KEY', label: 'xAI' },
  { key: 'MISTRAL_API_KEY', label: 'Mistral' },
  { key: 'DEEPSEEK_API_KEY', label: 'DeepSeek' },
  { key: 'OPENAI_BASE_URL', label: 'OpenAI Base URL' },
]

export interface EmbeddedServerSnapshot {
  /** Directory the agent works in. */
  workspace: string
  /** Provider credentials passed to the server process. */
  envVars: Record<string, string>
  /** Start the server automatically when the app launches. */
  autoStart: boolean
}

class EmbeddedServerStore {
  private _workspace = ''
  private _envVars: Record<string, string> = {}
  private _autoStart = true
  private _listeners = new Set<() => void>()
  private _snapshot: EmbeddedServerSnapshot

  constructor() {
    try {
      this._workspace = localStorage.getItem(STORAGE_KEY_WORKSPACE) || ''
    } catch {
      this._workspace = ''
    }
    try {
      const raw = localStorage.getItem(STORAGE_KEY_ENV_VARS)
      const parsed = raw ? JSON.parse(raw) : {}
      this._envVars = parsed && typeof parsed === 'object' ? (parsed as Record<string, string>) : {}
    } catch {
      this._envVars = {}
    }
    try {
      this._autoStart = localStorage.getItem(STORAGE_KEY_AUTO_START) !== 'false'
    } catch {
      this._autoStart = true
    }
    this._snapshot = this._buildSnapshot()
  }

  get workspace() {
    return this._workspace
  }

  get envVars() {
    return this._envVars
  }

  get autoStart() {
    return this._autoStart
  }

  /** Only the credentials that are actually filled in. */
  get activeEnvVars(): Record<string, string> {
    const result: Record<string, string> = {}
    for (const [key, value] of Object.entries(this._envVars)) {
      const trimmed = (value ?? '').trim()
      if (key.trim() && trimmed) result[key.trim()] = trimmed
    }
    return result
  }

  setWorkspace(path: string) {
    this._workspace = path
    try {
      localStorage.setItem(STORAGE_KEY_WORKSPACE, path)
    } catch {
      /* ignore */
    }
    this._notify()
  }

  setEnvVar(key: string, value: string) {
    const next = { ...this._envVars }
    if (!value.trim()) delete next[key]
    else next[key] = value
    this._envVars = next
    try {
      localStorage.setItem(STORAGE_KEY_ENV_VARS, JSON.stringify(next))
    } catch {
      /* ignore */
    }
    this._notify()
  }

  setAutoStart(value: boolean) {
    this._autoStart = value
    try {
      localStorage.setItem(STORAGE_KEY_AUTO_START, String(value))
    } catch {
      /* ignore */
    }
    this._notify()
  }

  subscribe = (fn: () => void) => {
    this._listeners.add(fn)
    return () => {
      this._listeners.delete(fn)
    }
  }

  getSnapshot = (): EmbeddedServerSnapshot => this._snapshot

  private _buildSnapshot(): EmbeddedServerSnapshot {
    return {
      workspace: this._workspace,
      envVars: { ...this._envVars },
      autoStart: this._autoStart,
    }
  }

  private _notify() {
    this._snapshot = this._buildSnapshot()
    this._listeners.forEach(fn => fn())
  }
}

export const embeddedStore = new EmbeddedServerStore()

export function useEmbeddedServerStore() {
  return useSyncExternalStore(embeddedStore.subscribe, embeddedStore.getSnapshot)
}
