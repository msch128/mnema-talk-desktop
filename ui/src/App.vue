<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import thirdPartyNotices from '../../THIRD_PARTY_NOTICES.md?raw'

interface Candidate { origin: string; community_id: string; api_version: number; login_available: boolean }
interface Game { platform: string; game: string | null; covers_monitor: boolean; exclusive_fullscreen_verified: boolean }
const address = ref('')
const busy = ref(false)
const candidate = ref<Candidate | null>(null)
const error = ref('')
let generation = 0
const games = ref('Wow.exe, GenshinImpact.exe')
const snapshot = ref<Game | null>(null)
const lastGame = ref<Game | null>(null)
const watching = ref(false)
const gameError = ref('')
const names = computed(() => games.value.split(',').map(value => value.trim()).filter(Boolean))
const invalidGames = computed(() => names.value.length === 0 || names.value.length > 32 || names.value.some(name =>
  name.length > 128 || name.includes('..') || !/^[a-zA-Z0-9][a-zA-Z0-9_. !-]*\.exe$/i.test(name)))
let timer: ReturnType<typeof setInterval> | undefined
let gameGeneration = 0
let polling = false
const mediaHooks = {
  webRtc: 'RTCPeerConnection' in globalThis,
  capture: typeof navigator.mediaDevices?.getDisplayMedia === 'function',
  frameTransform: 'RTCRtpScriptTransform' in globalThis
}
// The instance's web app signs every media frame with Ed25519 (WebCrypto);
// without it, voice in web-client mode cannot be end-to-end encrypted.
const ed25519 = ref<boolean | null>(null)
async function probeEd25519(): Promise<boolean> {
  try {
    await globalThis.crypto.subtle.generateKey({ name: 'Ed25519' }, false, ['sign', 'verify'])
    return true
  } catch {
    return false
  }
}
void probeEd25519().then(result => { ed25519.value = result })
// Since mnema-talk 0.8 the instance's web app sends a screen share as up to
// three layers from one capture, so viewers can pick a quality. Each layer is
// an ordinary sender whose encoding parameters scale the picture down and can
// be switched off; no RTP simulcast is involved.
const layers = ref<boolean | null>(null)
async function probeLayers(): Promise<boolean> {
  if (!('RTCPeerConnection' in globalThis)) return false
  let pc: RTCPeerConnection | undefined
  try {
    pc = new RTCPeerConnection()
    const { sender } = pc.addTransceiver('video', { direction: 'sendonly', sendEncodings: [{ scaleResolutionDownBy: 2, active: false }] })
    const encoding = sender.getParameters().encodings?.[0]
    return encoding?.scaleResolutionDownBy === 2 && encoding.active === false
  } catch {
    return false
  } finally {
    pc?.close()
  }
}
void probeLayers().then(result => { layers.value = result })
const gameStatus = computed(() => snapshot.value?.platform === 'unsupported_probe_host'
  ? 'This game check needs Windows.' : snapshot.value?.game ?? 'No approved game in the foreground.')
watch(address, () => { candidate.value = null; error.value = '' })

async function inspectServer() {
  if (busy.value) return
  if (!address.value.trim() || new TextEncoder().encode(address.value).length > 2048) {
    error.value = 'Enter a server address of at most 2048 bytes.'
    return
  }
  const own = ++generation
  busy.value = true; candidate.value = null; error.value = ''
  try {
    const result = await invoke<Candidate>('inspect_server', { address: address.value })
    if (own === generation) candidate.value = result
  } catch {
    if (own === generation) error.value = 'No compatible Mnema discovery reachable over HTTPS. Check the address, the certificate and the server version.'
  } finally {
    if (own === generation) busy.value = false
  }
}

async function pollGame() {
  if (polling || !watching.value) return
  const own = gameGeneration
  polling = true
  try {
    const result = await invoke<Game>('inspect_game', { names: names.value })
    if (own === gameGeneration && watching.value) {
      snapshot.value = result
      gameError.value = ''
      if (result.game) lastGame.value = result
      if (result.platform === 'unsupported_probe_host') {
        watching.value = false
        clearInterval(timer)
        timer = undefined
      }
    }
  } catch {
    if (own === gameGeneration && watching.value) {
      snapshot.value = null
      lastGame.value = null
      gameError.value = 'The local game check failed. Stop the check and start it again.'
    }
  } finally { polling = false }
}

function toggleWatching() {
  if (!watching.value && invalidGames.value) return
  gameGeneration++
  watching.value = !watching.value
  if (timer !== undefined) clearInterval(timer)
  timer = undefined
  snapshot.value = null
  lastGame.value = null
  gameError.value = ''
  if (watching.value) {
    timer = setInterval(() => { void pollGame() }, 1000)
    void pollGame()
  }
}

onUnmounted(() => {
  generation++; gameGeneration++; watching.value = false
  if (timer !== undefined) clearInterval(timer)
})
</script>

<template>
  <main>
    <header><span class="brand-mark" aria-hidden="true">M</span><h1>Mnema Talk</h1><span class="badge">Desktop · Development</span></header>
    <section aria-labelledby="connect-title">
      <h2 id="connect-title">Check community</h2>
      <form @submit.prevent="inspectServer">
        <label for="server">Server address</label>
        <div class="row"><input id="server" v-model="address" type="text" placeholder="https://talk.example.com" autocomplete="off" spellcheck="false" maxlength="2048" :disabled="busy"><button :disabled="busy || !address.trim()">{{ busy ? 'Checking …' : 'Check connection' }}</button></div>
      </form>
      <p v-if="error" role="alert">{{ error }}</p>
      <dl v-if="candidate"><dt>Target server</dt><dd>{{ candidate.origin }}</dd><dt>Community ID</dt><dd>{{ candidate.community_id }}</dd><dt>API contract</dt><dd>{{ candidate.api_version }}</dd></dl>
      <p class="help">Public server data only. Sign-in and voice first need a reviewed session and E2EE scheme.</p>
    </section>
    <section aria-labelledby="game-title">
      <h2 id="game-title">Detect the active game</h2>
      <label for="games">Executable files, separated by commas</label>
      <div class="row"><input id="games" v-model="games" :disabled="watching" spellcheck="false" :aria-invalid="invalidGames" :aria-describedby="invalidGames ? 'games-validation' : undefined"><button type="button" :disabled="!watching && invalidGames" @click="toggleWatching">{{ watching ? 'Stop check' : 'Start game check' }}</button></div>
      <p v-if="invalidGames" id="games-validation" role="alert">Enter 1 to 32 executable files ending in .exe. Start with a letter or digit, then spaces, dots, hyphens, underscores or exclamation marks are allowed; no paths and no consecutive dots. At most 128 characters per name.</p>
      <p v-if="gameError" role="alert">{{ gameError }}</p>
      <p class="status" aria-live="polite">{{ gameStatus }}</p>
      <p v-if="lastGame" class="help">Last detected locally: {{ lastGame.game }}. Monitor-filling geometry: {{ lastGame.covers_monitor ? 'yes' : 'no' }}. Exclusive fullscreen: not yet proven.</p>
      <p class="help">After starting, switch to the game, then check the result here. Only the foreground program is read; the window title and the full file path are neither shown nor sent.</p>
    </section>
    <section aria-labelledby="media-title">
      <h2 id="media-title">Runtime features</h2>
      <ul class="runtime-list"><li>WebRTC-API <span>{{ mediaHooks.webRtc ? 'present' : 'missing' }}</span></li><li>Screen capture API <span>{{ mediaHooks.capture ? 'present' : 'missing' }}</span></li><li>Encoded-Frame-Transform-API <span>{{ mediaHooks.frameTransform ? 'present' : 'missing' }}</span></li><li data-testid="ed25519">Ed25519 (WebCrypto, frame signatures) <span>{{ ed25519 === null ? 'checking' : ed25519 ? 'present' : 'missing' }}</span></li><li data-testid="layers">Sender parameters for screen layers (scaling, switching off) <span>{{ layers === null ? 'checking' : layers ? 'present' : 'missing' }}</span></li></ul>
      <p class="help">API availability proves neither capture, E2EE nor hardware encoding. This client does not start a capture.</p>
    </section>
    <details><summary>Open-source licenses and dependencies</summary><pre>{{ thirdPartyNotices }}</pre></details>
    <footer>Probe, not a released 0.7 client. Without voice, neither the widget nor the overlay appears.</footer>
  </main>
</template>
