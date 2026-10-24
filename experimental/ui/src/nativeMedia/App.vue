<script setup lang="ts">
import { computed, onUnmounted, ref, toRaw } from 'vue'
import { createNativeMediaBridge } from './native-media-invoke'
import { emptyReport, SyntheticLoopback, type Report } from './loopback'
const canvas = ref<HTMLCanvasElement>()
const video = ref<HTMLVideoElement>()
const report = ref<Report>(emptyReport(0, true))
report.value.phase = 'idle'
const previous = ref<Report[]>([])
const nativeBridge = createNativeMediaBridge()
let session: SyntheticLoopback | undefined
let run = 0
const running = computed(() => !['idle', 'stopped'].includes(report.value.phase))
async function start(mode: 'forward' | 'tamper' | 'revoke' | 'close') {
  await session?.stop()
  if (report.value.run) previous.value = [structuredClone(toRaw(report.value)), ...previous.value].slice(0, 4)
  if (!canvas.value || !video.value) return
  session = new SyntheticLoopback(canvas.value, video.value, true, ++run, value => { report.value = value }, true, mode, nativeBridge)
  report.value = emptyReport(run, true)
  await session.start()
}
async function stop() { await session?.stop() }
onUnmounted(() => { void session?.stop() })
</script>
<template>
  <main>
    <header><span class="mark">M</span><h1>Mnema Talk</h1><span>Native authenticated SFU test</span></header>
    <p>Two native test accounts in the WebView, a real authenticated Go/Pion SFU path on loopback. Synthetic video and 440 Hz audio only; random local fixture keys. No community credentials, no physical capture or storage.</p>
    <div class="buttons"><button :disabled="running" @click="start('forward')">SFU data flow</button><button :disabled="running" @click="start('tamper')">Check tampering</button><button :disabled="running" @click="start('close')">Close publisher transport</button><button :disabled="!running" @click="stop">Stop</button></div>
    <p class="status">Run {{ report.run }} · {{ report.fixtureMode }} · {{ report.phase }} · {{ report.seconds }} s · Data flow {{ report.passedSyntheticFlow ? 'proven' : 'not proven' }} · Fault case {{ report.passedFaultScenario ? 'proven' : 'not proven' }}</p>
    <div class="previews"><div><label>Synthetic source</label><canvas ref="canvas"></canvas></div><div><label>Received through the SFU</label><video ref="video" autoplay muted playsinline></video></div></div>
    <section><h2>Measurements</h2><pre>{{ JSON.stringify(report, null, 2) }}</pre></section>
    <details v-if="previous.length"><summary>Previous runs</summary><pre>{{ JSON.stringify(previous, null, 2) }}</pre></details>
    <footer>Developer fixture with native test accounts and session-bound signaling. The native integration is qualified separately; local fixture keys are not MLS member keys. A tampered frame is changed after encryption and must be rejected as unauthenticated; this measurement is kept as an error. Removal checks transport and SFU membership only, not MLS key revocation. No production E2EE, device trust, persistence, platform or gaming promise. SDP, ICE candidates, frames and keys are neither shown nor recorded. Audio output stays muted.</footer>
  </main>
</template>
