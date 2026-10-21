import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { MAX_BADGE, MIN_GAP_MS, ToastGate, badgeCount, cleanText, validId } from '../src/core/notify.ts'

test('toast text is flattened to plain, bounded text', () => {
  assert.equal(cleanText('a\nb\tc\u0000d‮e', 50), 'a b c d e')
  assert.equal(cleanText('  spaced   out ', 50), 'spaced out')
  const cut = cleanText('x'.repeat(500), 20)
  assert.equal(Array.from(cut).length, 20)
  assert.ok(cut.endsWith('…'))
  assert.equal(cleanText('é'.repeat(10), 10), 'é'.repeat(10))
  assert.equal(cleanText('\u{1F600}'.repeat(30), 5), `${'\u{1F600}'.repeat(4)}…`)
})

test('only plain identifiers may name the message of a toast', () => {
  assert.ok(validId('0b1e2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d'))
  for (const bad of ['', 'a b', '../x', 'a;b', 'a'.repeat(65), 'é']) assert.equal(validId(bad), false, bad)
})

test('the badge is a bounded whole number', () => {
  assert.equal(badgeCount(3.7), 3)
  assert.equal(badgeCount(-4), 0)
  assert.equal(badgeCount(5000), MAX_BADGE)
  assert.equal(badgeCount(Number.NaN), 0)
  assert.equal(badgeCount(Number.POSITIVE_INFINITY), 0)
})

test('toasts are rate limited', () => {
  const gate = new ToastGate()
  assert.equal(gate.allow(1000), true)
  assert.equal(gate.allow(1000 + MIN_GAP_MS - 1), false)
  assert.equal(gate.allow(1000 + MIN_GAP_MS), true)
})

test('the notification commands are allowed in the instance window only, and the capability is announced there only', () => {
  const ipc = readFileSync(new URL('../src/main/ipc.ts', import.meta.url), 'utf8')
  for (const command of ['desktop_notify_open', 'desktop_notify', 'desktop_notify_permission', 'desktop_notify_request_permission', 'desktop_set_badge']) {
    const at = ipc.indexOf(`case '${command}':`)
    assert.ok(at > 0, command)
    assert.match(ipc.slice(at, at + 140), /only\('instance'\)/, command)
  }
  assert.match(ipc, /capabilities: from\.role === 'instance' \? \['notify'\] : \[\]/)
})
