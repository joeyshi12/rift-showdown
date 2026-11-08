// Headless end-to-end tests for Rift Online. Starts nothing itself: expects the
// server at BASE (default http://127.0.0.1:8787). Run: npm test  (see README)
import { chromium, devices } from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';

const BASE = process.env.BASE || 'http://127.0.0.1:8787';
const OUT = 'screenshots';
fs.mkdirSync(OUT, { recursive: true });
const browser = await chromium.launch({ args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist'] });
const errors = [];
const pages = [];
let passed = 0;

const PASSWORD = 'e2e password long enough';
function watch(name, page) {
  pages.push([name, page]);
  page.on('pageerror', e => errors.push(`${name}: ${e.message}`));
  // 4xx answers from /api are expected in the validation steps.
  page.on('console', m => { if (m.type() === 'error' && !/Failed to load resource/.test(m.text())) errors.push(`${name} console: ${m.text()}`); });
}
// A fresh browser profile (own cookies), not signed in.
async function guest(name) {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 900 } });
  await ctx.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: BASE });
  const page = await ctx.newPage();
  watch(name, page);
  await page.goto(BASE);
  await page.waitForFunction(() => window.__state && window.__state().connected, null, { timeout: 20000 });
  return page;
}
// Sign up through the dialog (or sign in if a previous run created the account).
async function signUp(page, name) {
  await page.click('.profile [data-act=auth-open][data-arg=up]');
  await page.fill('#su-name', name);
  await page.fill('#su-email', `${name.toLowerCase()}@e2e.test`);
  await page.fill('#su-pw', PASSWORD);
  await page.click('#au-go');
  await page.waitForFunction(() => window.__state().signed_in || document.querySelector('#au-err')?.textContent, null, { timeout: 15000 });
  if (!(await st(page)).signed_in) {
    await page.click('[data-act=auth-switch][data-arg=in]');
    await page.fill('#si-email', `${name.toLowerCase()}@e2e.test`);
    await page.fill('#si-pw', PASSWORD);
    await page.click('#au-go');
  }
  await page.waitForFunction(n => window.__state().signed_in && window.__state().name === n && window.__state().connected, name, { timeout: 15000 });
}
async function newPlayer(name) {
  const page = await guest(name);
  await signUp(page, name);
  return page;
}
const st = p => p.evaluate(() => window.__state());
// Waits for the app to boot first: after a navigation, window.__state appears once the wasm is loaded.
const until = async (p, fn, arg, timeout = 15000) => {
  await p.waitForFunction(() => typeof window.__state === 'function', null, { timeout });
  return p.waitForFunction(fn, arg, { timeout });
};
const shot = (p, n) => p.screenshot({ path: `${OUT}/${n}.png` });
async function step(name, fn) {
  const t = Date.now();
  try { await fn(); passed++; console.log(`✔ ${name} (${Date.now() - t} ms)`); }
  catch (e) { console.log(`✘ ${name}\n   ${e.message.split('\n').slice(0, 12).join('\n   ')}`); errors.push(`${name}: ${e.message.split('\n')[0]}`);
    for (const [n, p] of pages) { if (!p.isClosed()) await p.screenshot({ path: `${OUT}/FAIL-${name.slice(0, 20).replace(/\W+/g, '_')}-${n}.png` }).catch(() => {}); } }
}

const alice = await guest('Alice');
let code;

await step('guests see sign in / sign up; online play asks for an account', async () => {
  assert.equal((await st(alice)).signed_in, false);
  assert.ok(await alice.isVisible('.profile [data-act=auth-open][data-arg=in]'));
  await alice.click('[data-act=create-room]');
  await until(alice, () => window.__state().auth_view === 'SignIn');
  assert.match(await alice.textContent('.au-reason'), /Sign in to play online/);
  await shot(alice, '00-sign-in');
  // Escape closes the dialog; no room was created.
  await alice.keyboard.press('Escape');
  await until(alice, () => window.__state().auth_view === null);
  assert.equal((await st(alice)).room, null);
  // Sign-up validation comes back from the server.
  await alice.click('.profile [data-act=auth-open][data-arg=up]');
  await alice.fill('#su-name', 'Alice');
  await alice.fill('#su-email', 'not-an-email');
  await alice.fill('#su-pw', PASSWORD);
  await alice.click('#au-go');
  await until(alice, () => /valid email/.test(document.querySelector('#au-err').textContent));
  assert.equal(await alice.inputValue('#su-name'), 'Alice', 'typed values survive the error');
  await alice.fill('#su-pw', 'short');
  await alice.click('#au-go');
  await until(alice, () => /at least 12/.test(document.querySelector('#au-err').textContent));
  await shot(alice, '00-sign-up');
  await alice.click('.au-x');
  await signUp(alice, 'Alice');
  assert.ok(await alice.isVisible('#acct-btn'));
});
const bob = await newPlayer('Bob');

await step('two accounts keep separate names; renaming is per account', async () => {
  await alice.click('#acct-btn');
  await alice.click('[data-act=acct-open]');
  await alice.fill('#acct-name', 'Bob');
  await alice.click('#au-go');
  await until(alice, () => /taken/.test(document.querySelector('#au-err').textContent));
  await alice.fill('#acct-name', 'Alicia');
  await alice.click('#au-go');
  await until(alice, () => window.__state().name === 'Alicia');
  await shot(alice, '00-account');
  assert.equal((await st(bob)).name, 'Bob');
  await alice.fill('#acct-name', 'Alice');
  await alice.click('#au-go');
  await until(alice, () => window.__state().name === 'Alice');
  await alice.click('.au-x');
  assert.equal(await alice.textContent('.acct-name'), 'Alice');
  assert.equal(await bob.textContent('.acct-name'), 'Bob');
});

await step('home screen renders with nav and panels', async () => {
  assert.equal((await st(alice)).screen, 'Home(Duel)');
  assert.ok(await alice.isVisible('text=Private room'));
  assert.ok(await alice.isVisible('.hero-art'));
  await shot(alice, '01-home');
});

await step("deck list shows Riot's preconstructed decks; import a deck code", async () => {
  await alice.click('[data-arg=Decks]');
  await until(alice, () => document.querySelectorAll('.deck-case').length >= 5);
  const names = await alice.$$eval('.dc-name', e => e.map(x => x.textContent));
  for (const n of ['Rumble Champion Deck', 'Fiora Champion Deck', 'Vex Champion Deck', 'Vi Champion Deck', 'Shen Showdown Deck']) assert.ok(names.includes(n), `${n} in ${names}`);
  // The "Add Riot deck" list offers only Riot's decks.
  const offered = await alice.$$eval('#starter-legend option', e => e.map(x => x.textContent));
  assert.equal(offered.length, 5);
  assert.ok(offered.every(o => /(Champion|Showdown) Deck/.test(o)), offered.join());
  // export a code from the first deck via the engine-compatible encoder, then import it
  await alice.click('.deck-case [data-act=deck-code]');
  const clip = await alice.evaluate(() => navigator.clipboard.readText());
  assert.match(clip, /^[A-Z2-7]+$/);
  await alice.click('[data-act=import-toggle]');
  await alice.fill('#imp-name', 'Imported deck');
  await alice.fill('#imp-text', clip);
  await alice.click('[data-act=import-deck]');
  await until(alice, () => [...document.querySelectorAll('.dc-name')].some(e => e.textContent === 'Imported deck'));
  await shot(alice, '02-decks');
});

await step('text decklist import with sections', async () => {
  await alice.click('[data-act=import-toggle]');
  await alice.fill('#imp-name', 'Text List');
  await alice.fill('#imp-text', 'Legend:\n1 Jinx, Loose Cannon\nMainDeck:\n3 Blazing Scorcher\nRunes:\n12 Fury Rune');
  await alice.click('[data-act=import-deck]');
  await until(alice, () => [...document.querySelectorAll('.dc-name')].some(e => e.textContent === 'Text List'));
});

await step('deck editor: filter, add cards, validation, save', async () => {
  await alice.click('.deck-case [data-act=deck-edit]');
  await until(alice, () => document.querySelectorAll('.ctile').length > 50);
  await alice.fill('#ed-q', 'scorcher');
  await until(alice, () => document.querySelectorAll('.ctile').length >= 1 && document.querySelectorAll('.ctile').length < 10);
  const before = await alice.evaluate(() => document.querySelector('.dsum b').textContent);
  await alice.click('.ctile');
  await until(alice, b => document.querySelector('.dsum b').textContent !== b, before);
  assert.ok(await alice.isVisible('.valid'));
  await alice.fill('#ed-q', '');
  await alice.selectOption('#ed-type', 'Battlefield');
  await until(alice, () => document.querySelectorAll('.ctile.land').length > 5);
  await alice.mouse.move(400, 300);
  await alice.hover('.ctile.land');
  await until(alice, () => !!document.querySelector('#preview .pv-name'));
  await shot(alice, '03-editor');
  await alice.click('[data-act=ed-save]');
  await alice.click('[data-act=ed-back]');
  await until(alice, () => window.__state().screen === 'Home(Decks)');
});

await step('sealed: open packs deterministically and auto-build', async () => {
  await alice.click('[data-arg=Sealed]');
  await alice.fill('#sealed-seed', 'E2E-SEED');
  await alice.click('[data-act=sealed-open]');
  await until(alice, () => document.querySelectorAll('.pack').length === 6);
  const n = await alice.evaluate(() => document.querySelectorAll('.pc').length);
  assert.equal(n, 84);
  const first = await alice.evaluate(() => document.querySelector('.pc').dataset.code);
  await alice.click('[data-act=sealed-open]');
  await until(alice, f => document.querySelector('.pc').dataset.code === f, first);
  await alice.waitForTimeout(800);
  await shot(alice, '04-sealed');
  await alice.click('[data-act=sealed-auto]');
  await until(alice, () => window.__state().screen === 'Editor');
  await alice.click('[data-act=ed-save]');
  await alice.click('[data-act=ed-back]');
});

await step('create room, join by code, chat', async () => {
  await alice.click('[data-arg=Duel]');
  await alice.selectOption('#cr-format', 'Bo3');
  await alice.check('#cr-public');
  await alice.click('[data-act=create-room]');
  await until(alice, () => window.__state().screen === 'Lobby' && window.__state().room);
  code = (await st(alice)).room;
  assert.match(code, /^[A-Z0-9]{6}$/);
  await bob.fill('#join-code', code.toLowerCase());
  await bob.click('[data-act=join-room]');
  await until(bob, () => window.__state().screen === 'Lobby' && window.__state().players === 2);
  await until(alice, () => window.__state().players === 2);
  await bob.fill('#chat-input', 'glhf!');
  await bob.press('#chat-input', 'Enter');
  await until(alice, () => [...document.querySelectorAll('.cl')].some(e => e.textContent.includes('glhf!')));
});

await step('public room browser lists the room', async () => {
  const carol = await guest('Carol');
  await carol.click('[data-arg=Rooms]');
  await until(carol, c => [...document.querySelectorAll('.rc')].some(e => e.textContent === c), code);
  await shot(carol, '05-rooms');
  await carol.close();
});

await step('both players load decks, ready up, match starts', async () => {
  for (const p of [alice, bob]) {
    // pick a standard starter deck (the last ones in the list are the defaults)
    const val = await p.evaluate(() => [...document.querySelectorAll('#lobby-deck option')].find(o => /(Champion|Showdown) Deck/.test(o.textContent)).value);
    await p.selectOption('#lobby-deck', val);
    await p.click('[data-act=lobby-use-deck]');
  }
  await until(alice, () => document.querySelectorAll('.seat-state').length === 2 && [...document.querySelectorAll('.seat-state')].every(e => /Deck loaded/.test(e.textContent)));
  await shot(alice, '06-lobby');
  await alice.click('[data-act=ready]');
  await bob.click('[data-act=ready]');
  await until(alice, () => window.__state().screen === 'Game');
  await until(bob, () => window.__state().screen === 'Game');
});

const spec = await guest('Sam');
await step('spectator joins live match by link', async () => {
  await spec.goto(`${BASE}/?room=${code}&spectate=1`);
  await until(spec, () => window.__state().screen === 'Game' && window.__state().game.spectator);
});

await step('setup: battlefields, turn order, mulligan', async () => {
  for (const p of [alice, bob]) {
    await p.waitForSelector('.bfopt');
    if (p === alice) await shot(p, '07-choose-battlefield');
    await p.click('.bfopt');
  }
  await until(alice, () => window.__state().game.stage.startsWith('ChooseOrder'));
  const s = await st(alice);
  const chooser = s.game.stage.includes('chooser: 0') ? alice : bob;
  await chooser.waitForSelector('[data-act=setup-order]');
  await shot(chooser, '08-turn-order');
  await chooser.click('[data-act=setup-order][data-arg=first]');
  await until(alice, () => window.__state().game.stage === 'Mulligan');
  await alice.waitForSelector('.mull');
  await alice.click('.mull');
  await shot(alice, '09-mulligan');
  await alice.click('[data-act=mull-confirm]');
  await bob.waitForSelector('[data-act=mull-confirm]');
  await bob.click('[data-act=mull-confirm]');
  await until(alice, () => window.__state().game.stage === 'Play');
  await until(bob, () => window.__state().game.stage === 'Play');
});

let active, other;
await step('active player draws, channels and drags a card from hand onto the base', async () => {
  const s = await st(alice);
  [active, other] = s.game.active === s.game.me ? [alice, bob] : [bob, alice];
  const before = await st(active);
  await active.keyboard.press('c'); // channel 1 extra rune
  await until(active, n => window.__state().game.runes === n + 1, before.game.runes);
  for (let i = 0; i < 6; i++) await active.keyboard.press('c');
  await active.waitForTimeout(400);
  const card = await active.$('.hc');
  const box = await card.boundingBox();
  const me = (await st(active)).game.me;
  const [tx, ty] = await active.evaluate(m => window.__zone({ Base: m }), me);
  await active.mouse.move(box.x + box.width / 2, box.y + 40);
  await active.mouse.down();
  await active.mouse.move(box.x + 30, box.y - 60, { steps: 4 });
  await active.mouse.move(tx, ty, { steps: 10 });
  await active.mouse.up();
  await until(active, () => window.__state().game.base >= 1 || /chain/i.test(window.__state().game.log || '') || /Played/.test(window.__state().game.log || ''));
  await active.waitForTimeout(1200);
  await shot(active, '10-played-card');
});

await step('opponent and spectator see the played card on the table', async () => {
  await until(other, () => /Played|Auto-paid/.test(window.__state().game.log || ''));
  await other.waitForTimeout(800);
  await shot(other, '11-opponent-view');
  await spec.waitForTimeout(500);
  await shot(spec, '12-spectator-view');
  const ss = await st(spec);
  assert.equal(ss.game.spectator, true);
});

await step('card context menu: exhaust toggles via menu', async () => {
  const me = (await st(active)).game.me;
  const [x, y] = await active.evaluate(m => window.__zone({ Runes: m }), me);
  // click the rune pool row's first card (left side of the row)
  await active.mouse.click(x - 30, y);
  const menu = await active.waitForSelector('#ctx', { timeout: 5000 }).catch(() => null);
  if (!menu) {
    await active.mouse.click(x, y);
    await active.waitForSelector('#ctx');
  }
  await shot(active, '13-context-menu');
  await active.keyboard.press('Escape');
});

await step('rewind undoes the last action and forward restores it', async () => {
  const before = await st(active);
  await active.keyboard.press('d');
  await until(active, n => window.__state().game.hand === n + 1, before.game.hand);
  await active.keyboard.press('Control+z');
  await until(active, n => window.__state().game.hand === n, before.game.hand);
  await active.keyboard.press('Control+y');
  await until(active, n => window.__state().game.hand === n + 1, before.game.hand);
});

await step('duel log lists newest lines at the bottom and stays scrolled there', async () => {
  const lines = () => [...document.querySelectorAll('#g-log .ll')].map(e => e.textContent);
  const before = await active.evaluate(lines);
  await active.keyboard.press('r');
  await until(active, n => document.querySelectorAll('#g-log .ll').length > n || /rolled a d20/i.test(document.querySelector('#g-log .ll:last-child')?.textContent || ''), before.length);
  await until(active, () => /rolled a d20/i.test(document.querySelector('#g-log .ll:last-child').textContent));
  await active.waitForTimeout(100);
  const pinned = await active.evaluate(() => { const e = document.getElementById('g-log'); return e.scrollTop + e.clientHeight >= e.scrollHeight - 30; });
  assert.ok(pinned, 'log is scrolled to the newest line');
});

await step('end turn passes to the opponent with auto steps', async () => {
  await active.click('[data-act=end-turn]');
  await until(other, () => { const s = window.__state().game; return s.active === s.me && s.phase === 'Action'; });
  await other.waitForTimeout(600);
  await shot(other, '14-opponent-turn');
});

await step('in-game chat and score adjust', async () => {
  await other.click('[data-act=panel][data-arg=chat]');
  // A focused field shows one lime ring (its border), not a border plus an outline.
  await other.click('#chat-input');
  const ring = await other.$eval('#chat-input', e => { const c = getComputedStyle(e); return { outline: c.outlineStyle, border: c.borderTopColor }; });
  assert.equal(ring.outline, 'none', 'no focus outline on top of the border');
  assert.equal(ring.border, 'rgb(204, 255, 0)', 'the border turns lime');
  await other.keyboard.press('Tab');
  await other.keyboard.press('Shift+Tab');
  assert.equal(await other.$eval('#chat-input', e => getComputedStyle(e).outlineStyle), 'none', 'also when focused from the keyboard');
  await other.fill('#chat-input', 'nice play');
  await other.press('#chat-input', 'Enter');
  await until(active, () => [...document.querySelectorAll('#chat-lines .cl')].some(e => e.textContent.includes('nice play')));
  await other.click('#g-me .pl-score');
  await other.click('[data-act=score][data-arg$="|1"]');
  await until(active, () => window.__state().game.scores.some(s => s === 1));
});

await step('concede shows victory/defeat and Bo3 next game', async () => {
  await other.click('[data-act=game-menu]');
  await other.click('[data-act=concede]');
  await active.waitForSelector('.result.win');
  await other.waitForSelector('.result.lose');
  await active.waitForTimeout(700);
  await shot(active, '15-victory');
  await active.click('[data-act=next-game]');
  await other.click('[data-act=next-game]');
  await active.waitForSelector('[data-act=side-lock]');
  await shot(active, '16-sideboarding');
  await active.click('[data-act=side-lock]');
  await other.click('[data-act=side-lock]');
  await until(alice, () => window.__state().game.stage === 'ChooseBattlefield');
  await other.click('[data-act=game-menu]');
  await other.click('[data-act=to-lobby]');
  await until(alice, () => window.__state().screen === 'Lobby');
  await until(spec, () => window.__state().screen === 'Lobby');
});

await step('matchmaking pairs two searching players', async () => {
  for (const p of [alice, bob]) {
    await p.click('[data-act=leave-room]');
    await until(p, () => window.__state().screen.startsWith('Home'));
    await p.click('[data-arg=Duel]');
    await p.click('[data-act=home-deck][title*="Champion Deck"]');
    await until(p, () => document.querySelector('.pick.on')?.title.includes('Champion Deck'));
  }
  await alice.click('[data-act=queue]');
  await alice.waitForSelector('.queue-pop');
  await shot(alice, '17-queue');
  await bob.click('[data-act=queue]');
  await until(alice, () => window.__state().screen === 'Game');
  await until(bob, () => window.__state().screen === 'Game');
  const a = await st(alice), b = await st(bob);
  assert.equal(a.room, b.room);
  await alice.click('[data-act=game-menu]');
  await alice.click('[data-act=exit-game]');
  await until(bob, () => window.__state().game === null || window.__state().screen === 'Lobby');
});

await step('goldfish solo practice with test setup, runs offline', async () => {
  const p = await guest('Goldie');
  await p.click('[data-arg=Solo]');
  await p.click('[data-act=goldfish]');
  await until(p, () => window.__state().game && window.__state().game.stage === 'Play');
  const s = await st(p);
  assert.equal(s.game.local, true);
  await p.keyboard.press('t');
  await p.waitForSelector('[data-act=token-make]');
  await p.click('[data-act=token-make]');
  await until(p, () => window.__state().game.base >= 1);
  // "Move to…" lights up each place the unit can go; clicking one moves it there.
  await p.waitForTimeout(600);
  const [bx, by] = await p.evaluate(() => window.__zone({ Base: 0 }));
  await p.mouse.click(bx, by);
  await p.waitForSelector('#ctx');
  assert.equal(await p.$$eval('#ctx .mi', e => e.filter(x => /^Move to Battlefield/.test(x.textContent)).length), 0, 'no per-battlefield duplicates');
  await p.click('#ctx [data-act=c-target]');
  await until(p, () => window.__state().game.targeting === 'Move' && window.__state().game.targets === 2);
  assert.match(await p.textContent('#g-target'), /Choose where to move/);
  await shot(p, '17-move-targets');
  const [ax, ay] = await p.evaluate(() => window.__zone({ Bf: [0, 0] }));
  await p.mouse.move(ax, ay);
  await p.mouse.click(ax, ay);
  await until(p, () => window.__state().game.base === 0 && window.__state().game.targeting === null);
  // Escape cancels a pending choice; clicking an empty spot cancels too.
  await p.waitForTimeout(600);
  await p.mouse.click(ax, ay);
  await p.waitForSelector('#ctx [data-act=c-target]');
  await p.click('#ctx [data-act=c-target]');
  await until(p, () => window.__state().game.targeting === 'Move');
  await p.keyboard.press('Escape');
  await until(p, () => window.__state().game.targeting === null);
  // Right after cancelling, a click on the same card opens its menu (not a double-click).
  await p.mouse.click(ax, ay);
  await p.waitForSelector('#ctx [data-act=c-target]');
  await p.click('#ctx [data-act=c-target]');
  await until(p, () => window.__state().game.targeting === 'Move');
  await p.mouse.click(bx, by);
  await until(p, () => window.__state().game.base === 1 && window.__state().game.targeting === null);
  await p.keyboard.press('e');
  await until(p, () => window.__state().game.turn === 2);
  await p.waitForTimeout(1500);
  await shot(p, '18-goldfish');
  await p.close();
});

await step('solo play controls both seats', async () => {
  const p = await guest('Solo');
  await p.click('[data-arg=Solo]');
  await p.click('[data-act=solo-play]');
  await p.waitForSelector('.bfopt');
  await p.click('.bfopt');
  await p.waitForSelector('.bfopt');
  await p.click('.bfopt');
  await p.waitForSelector('[data-act=setup-order]');
  await p.click('[data-act=setup-order][data-arg=first]');
  await p.click('[data-act=mull-confirm]');
  await p.waitForSelector('[data-act=mull-confirm]');
  await p.click('[data-act=mull-confirm]');
  await until(p, () => window.__state().game.stage === 'Play');
  const a0 = (await st(p)).game.active;
  await p.keyboard.press('e');
  await until(p, a => { const g = window.__state().game; return g.active !== a && g.me === g.active; }, a0);
  await p.waitForTimeout(1500);
  await shot(p, '19-solo-play');
  await p.close();
});

await step('opening the same account in a second tab retires the first', async () => {
  const tab2 = await alice.context().newPage();
  watch('Alice tab 2', tab2);
  await tab2.goto(BASE);
  await tab2.waitForFunction(() => window.__state && window.__state().connected && window.__state().signed_in, null, { timeout: 20000 });
  await until(alice, () => !!document.querySelector('[data-act=auth-resume]'));
  await alice.waitForTimeout(1500);
  assert.equal((await st(alice)).connected, false, 'first tab does not reconnect by itself');
  assert.equal((await st(tab2)).connected, true);
  await shot(alice, '20-other-tab');
  await tab2.close();
  await alice.click('[data-act=auth-resume]');
  await until(alice, () => window.__state().connected && !document.querySelector('[data-act=auth-resume]'));
});

await step('sign out returns to guest', async () => {
  await alice.click('#acct-btn');
  await alice.click('[data-act=acct-sign-out]');
  await until(alice, () => !window.__state().signed_in && window.__state().connected);
  assert.ok(await alice.isVisible('.profile [data-act=auth-open][data-arg=up]'));
});

await step('forgot password: email link, new password, sign in', async () => {
  // dev-server.sh has no SMTP, so the server writes the email to its log.
  const LOG = process.env.SERVER_LOG || '../target/dev-server/server.log';
  const linkFor = () => [...fs.readFileSync(LOG, 'utf8').matchAll(/\/\?reset=([0-9a-f]{64})/g)].map(m => m[1]).at(-1);
  const before = linkFor();
  await alice.click('.profile [data-act=auth-open][data-arg=in]');
  await alice.fill('#si-email', 'alice@e2e.test');
  await alice.click('[data-act=auth-forgot]');
  await until(alice, () => window.__state().auth_view === 'Forgot');
  assert.equal(await alice.inputValue('#fp-email'), 'alice@e2e.test', 'email carried over');
  await shot(alice, '21-forgot');
  await alice.click('#au-go');
  await alice.waitForSelector('text=Check your email');
  let token;
  for (let i = 0; i < 50 && (!token || token === before); i++) { await alice.waitForTimeout(100); token = linkFor(); }
  assert.ok(token && token !== before, 'reset link written to the server log');
  await alice.goto(`${BASE}/?reset=${token}`);
  await until(alice, () => window.__state().auth_view === 'Reset');
  assert.ok(!(await alice.evaluate(() => location.search)).includes('reset'), 'token removed from the address bar');
  await alice.fill('#rp-pw', 'short');
  await alice.click('#au-go');
  await until(alice, () => /at least 12/.test(document.querySelector('#au-err').textContent));
  await shot(alice, '22-reset');
  await alice.fill('#rp-pw', 'a new e2e password');
  await alice.click('#au-go');
  await until(alice, () => window.__state().signed_in && window.__state().name === 'Alice' && window.__state().connected);
  // Used links stop working.
  await alice.click('#acct-btn');
  await alice.click('[data-act=acct-sign-out]');
  await until(alice, () => !window.__state().signed_in);
  await alice.goto(`${BASE}/?reset=${token}`);
  await until(alice, () => window.__state().auth_view === 'Reset');
  await alice.fill('#rp-pw', 'another long password');
  await alice.click('#au-go');
  await until(alice, () => /invalid or has expired/.test(document.querySelector('#au-err').textContent));
  // The old password no longer works; the new one does.
  await alice.click('[data-act=auth-forgot]');
  await alice.click('[data-act=auth-switch][data-arg=in]');
  await alice.fill('#si-email', 'alice@e2e.test');
  await alice.fill('#si-pw', PASSWORD);
  await alice.click('#au-go');
  await until(alice, () => /Wrong email or password/.test(document.querySelector('#au-err').textContent));
  await alice.fill('#si-pw', 'a new e2e password');
  await alice.click('#au-go');
  await until(alice, () => window.__state().signed_in);
});

await step('installable PWA: manifest, icons, service worker, offline shell', async () => {
  const ctx = await browser.newContext();
  const p = await ctx.newPage();
  await p.goto(`${BASE}/`);
  await p.waitForSelector('.hero-art');
  const m = await p.evaluate(async () => {
    const href = document.querySelector('link[rel=manifest]').href;
    const r = await fetch(href);
    return { type: r.headers.get('content-type'), json: await r.json() };
  });
  assert.match(m.type, /manifest\+json/);
  assert.equal(m.json.display, 'standalone');
  assert.ok(m.json.icons.some((i) => i.sizes === '512x512' && i.purpose === 'maskable'));
  for (const i of m.json.icons) assert.equal((await p.request.get(`${BASE}${i.src}`)).status(), 200);
  await p.waitForFunction(async () => (await navigator.serviceWorker.ready).active?.state === 'activated', null, { timeout: 15000 });
  await p.reload();
  await p.waitForFunction(() => !!navigator.serviceWorker.controller);
  await p.waitForSelector('.hero-art');
  // Offline: the cached shell still boots and solo play works.
  await ctx.setOffline(true);
  await p.reload();
  await p.waitForSelector('.home.play', { timeout: 15000 });
  await p.click('[data-arg=Solo]');
  await p.click('[data-act=goldfish]');
  await until(p, () => window.__state().game && window.__state().game.stage === 'Play');
  await ctx.setOffline(false);
  await ctx.close();
});

await step('phone: touch tap opens the card menu, portrait table, landscape HUD', async () => {
  const ctx = await browser.newContext({ ...devices['Pixel 7'] });
  const p = await ctx.newPage();
  await p.goto(`${BASE}/`);
  await p.waitForSelector('.hero-art');
  await p.tap('[data-arg=Solo]');
  await p.tap('[data-act=goldfish]');
  await until(p, () => window.__state().game && window.__state().game.stage === 'Play');
  await p.waitForSelector('.hc');
  const box = await (await p.$('.hc')).boundingBox();
  await p.touchscreen.tap(box.x + box.width / 2, box.y + 30);
  await p.waitForSelector('#ctx .mi');
  assert.ok((await p.$eval('#preview', (e) => e.innerHTML.length)) > 0, 'tap shows the card details');
  // Bottom sheet stays on screen, nothing scrolled.
  const sheet = await (await p.$('#ctx')).boundingBox();
  const vp = p.viewportSize();
  assert.ok(sheet.x >= 0 && sheet.x + sheet.width <= vp.width && sheet.y + sheet.height <= vp.height);
  assert.equal(await p.$eval('#screen', (e) => e.scrollTop), 0);
  await p.screenshot({ path: 'screenshots/30-phone-menu.png' });
  await p.tap('#ctx .mi');
  await p.setViewportSize({ width: 915, height: 412 });
  await p.waitForTimeout(400);
  const tools = await (await p.$('.tools')).boundingBox();
  const phase = await (await p.$('.ph-btn')).boundingBox();
  assert.ok(tools.y + tools.height <= phase.y, 'tools sit above the phase button');
  await p.screenshot({ path: 'screenshots/31-phone-landscape.png' });
  await ctx.close();
});

await step("old generated starter decks nobody touched are swapped for Riot's decks; edited ones stay", async () => {
  const p = await guest('Olive');
  // A starter deck as older versions generated it on a first visit.
  const old = { name: 'Jinx, Loose Cannon Starter', legend: 'OGN-251', champion: 'OGN-030', main: [['OGN-001', 3], ['OGN-002', 3]], runes: [['OGN-007', 6], ['OGN-089', 6]], battlefields: ['OGN-275', 'OGN-277', 'OGN-278'], side: [] };
  const decks = [
    { id: 'abc123OGN-251', deck: old, sealed: null, updated: 0 },
    { id: 'def456OGN-251', deck: { ...old, name: 'My Jinx' }, sealed: null, updated: 0 },
    { id: 'ghi789OGN-251', deck: old, sealed: null, updated: 1700000000000 },
  ];
  await p.evaluate(d => localStorage.setItem('rift.decks.v1', JSON.stringify(d)), decks);
  await p.reload();
  await until(p, () => window.__state().decks === 7);
  const saved = JSON.parse(await p.evaluate(() => localStorage.getItem('rift.decks.v1')));
  const ids = saved.map(d => d.id);
  assert.ok(!ids.includes('abc123OGN-251'), 'untouched generated starter removed');
  assert.ok(ids.includes('def456OGN-251'), 'renamed deck kept');
  assert.ok(ids.includes('ghi789OGN-251'), 'edited deck kept');
  assert.equal(saved.filter(d => /(Champion|Showdown) Deck$/.test(d.deck.name)).length, 5, "Riot's five decks added");
  await p.context().close();
});

// ---------------------------------------------------------------- 2v2 and free-for-all
const four = [];
for (const n of ['Tess', 'Uma', 'Vic', 'Wes']) four.push(await newPlayer(n));
const [tess] = four;
let code4;
// Seat number -> page, read from each page's own view.
const bySeat = async () => { const m = {}; for (const p of four) { const s = await st(p); if (s.game && !s.game.spectator) m[s.game.me] = p; } return m; };
const allUntil = (fn, arg) => Promise.all(four.map(p => until(p, fn, arg, 20000)));

await step('2v2 room: four players, two teams, different starter decks', async () => {
  await tess.click('[data-arg=Duel]');
  await tess.selectOption('#cr-mode', 'teams');
  await tess.click('[data-act=create-room]');
  await until(tess, () => window.__state().screen === 'Lobby' && window.__state().room);
  code4 = (await st(tess)).room;
  for (const p of four.slice(1)) {
    await p.fill('#join-code', code4);
    await p.click('[data-act=join-room]');
  }
  await allUntil(() => window.__state().players === 4);
  assert.equal(await tess.$$eval('.seats.teams .team', e => e.length), 2);
  assert.equal(await tess.inputValue('#rs-points'), '11');
  // Each player takes a different starter deck (teammates can't share a legend).
  for (const [i, p] of four.entries()) {
    const val = await p.evaluate(i => [...document.querySelectorAll('#lobby-deck option')].filter(o => /(Champion|Showdown) Deck/.test(o.textContent))[i].value, i);
    await p.selectOption('#lobby-deck', val);
    await p.click('[data-act=lobby-use-deck]');
  }
  await until(tess, () => [...document.querySelectorAll('.seat-state')].filter(e => /Deck loaded/.test(e.textContent)).length === 4);
  await shot(tess, '40-lobby-2v2');
  for (const p of four) await p.click('[data-act=ready]');
  await allUntil(() => window.__state().screen === 'Game');
});

await step('2v2 setup: roll, first player brings no battlefield, three battlefields, mulligan', async () => {
  await until(tess, () => window.__state().game.stage.startsWith('ChooseOrder'));
  const seats = await bySeat();
  const chooser = Number((await st(tess)).game.stage.match(/chooser: (\d)/)[1]);
  await seats[chooser].click('[data-act=setup-order][data-arg=first]');
  await allUntil(() => window.__state().game.stage === 'ChooseBattlefield');
  const first = (await st(tess)).game.first;
  assert.equal(first, chooser);
  assert.match(await seats[first].textContent('#g-overlay'), /don't bring a battlefield/);
  for (const [s, p] of Object.entries(seats)) if (Number(s) !== first) await p.click('.bfopt');
  await allUntil(() => window.__state().game.stage === 'Mulligan');
  assert.equal((await st(tess)).game.nbf, 3);
  for (const p of four) { await p.waitForSelector('[data-act=mull-confirm]'); await p.click('[data-act=mull-confirm]'); }
  await allUntil(() => window.__state().game.stage === 'Play');
});

await step('2v2 table: four boards, teammate hand visible, turns alternate teams', async () => {
  const seats = await bySeat();
  const s0 = await st(seats[0]);
  assert.equal(s0.game.mode, 'Teams');
  // Seat 0 sees teammate seat 2's hand, but not seats 1 and 3.
  assert.equal(s0.game.visible_hands[2], s0.game.hands[2]);
  assert.equal(s0.game.visible_hands[1], 0);
  assert.equal(await seats[0].$$eval('#g-others .plate.mini', e => e.length), 3);
  assert.equal(await seats[0].$$eval('#g-others .plate.mini.ally', e => e.length), 1);
  const first = s0.game.first;
  assert.equal(s0.game.hands[first], 4, 'first player skipped the first draw');
  for (const p of four) await until(p, () => window.__state().game.textures_pending === 0, null, 20000).catch(() => {});
  await seats[first].waitForTimeout(800);
  await shot(seats[first], '41-table-2v2');
  await shot(seats[(first + 1) % 4], '42-table-2v2-opponent');
  // Ally's hand opens from its plate.
  await seats[0].click('#g-others .plate.mini.ally [data-act=z-view]');
  await seats[0].waitForSelector('.modal-bg .pk img');
  await shot(seats[0], '43-teammate-hand');
  await seats[0].click('.modal-bg [data-act=close-modal] >> nth=-1');
  await seats[first].click('[data-act=end-turn]');
  const next = (first + 1) % 4;
  await until(seats[next], () => { const g = window.__state().game; return g.active === g.me && g.phase === 'Action'; });
  assert.notEqual(next % 2, first % 2, 'the other team plays next');
});

await step('2v2 shared score and team victory', async () => {
  const seats = await bySeat();
  await seats[0].click('#g-me .pl-score');
  await seats[0].click('[data-act=score][data-arg$="|1"]');
  await until(seats[2], () => { const s = window.__state().game.scores; return s[0] === 1 && s[2] === 1; });
  await seats[1].click('[data-act=game-menu]');
  await seats[1].click('[data-act=concede]');
  for (const s of [0, 2]) await seats[s].waitForSelector('.result.win');
  for (const s of [1, 3]) await seats[s].waitForSelector('.result.lose');
  await seats[0].waitForTimeout(600);
  await shot(seats[0], '44-victory-2v2');
  await seats[0].click('[data-act=to-lobby]');
  await allUntil(() => window.__state().screen === 'Lobby');
});

await step('free-for-all: four players, leaving player concedes, the rest play on', async () => {
  await tess.selectOption('#rs-mode', 'ffa');
  await until(tess, () => document.querySelector('#rs-points')?.value === '8' && !document.querySelector('.seats.teams'));
  for (const p of four) { await p.waitForSelector('[data-act=ready]'); await p.click('[data-act=ready]'); }
  await allUntil(() => window.__state().screen === 'Game' && window.__state().game.stage.startsWith('ChooseOrder'));
  let seats = await bySeat();
  const chooser = Number((await st(tess)).game.stage.match(/chooser: (\d)/)[1]);
  await seats[chooser].click('[data-act=setup-order][data-arg=first]');
  await allUntil(() => window.__state().game.stage === 'ChooseBattlefield');
  const first = (await st(tess)).game.first;
  for (const [s, p] of Object.entries(seats)) if (Number(s) !== first) await p.click('.bfopt');
  await allUntil(() => window.__state().game.stage === 'Mulligan');
  for (const p of four) { await p.waitForSelector('[data-act=mull-confirm]'); await p.click('[data-act=mull-confirm]'); }
  await allUntil(() => window.__state().game.stage === 'Play');
  const s = await st(seats[first]);
  assert.equal(s.game.mode, 'FreeForAll');
  assert.equal(s.game.visible_hands.filter((n, i) => i !== s.game.me && n > 0).length, 0, 'free-for-all hands are private');
  await seats[first].waitForTimeout(1200);
  await shot(seats[first], '45-table-ffa');
  // The active player leaves: their turn passes on and the match continues.
  const leaver = seats[first];
  await leaver.click('[data-act=game-menu]');
  await leaver.click('[data-act=exit-game]');
  const rest = Object.entries(seats).filter(([k]) => Number(k) !== first).map(([, p]) => p);
  for (const p of rest) await until(p, f => { const g = window.__state().game; return g && g.conceded[f] && g.active === (f + 1) % 4 && g.stage === 'Play'; }, first);
  await shot(rest[0], '46-ffa-after-leave');
  // Two more concede; the last one standing wins.
  seats = await bySeat();
  const order = [1, 2, 3].map(k => (first + k) % 4);
  for (const k of order.slice(0, 2)) { await seats[k].click('[data-act=game-menu]'); await seats[k].click('[data-act=concede]'); }
  await seats[order[2]].waitForSelector('.result.win');
  await shot(seats[order[2]], '47-ffa-victory');
});

await step('three-player free-for-all: two battlefields, centred board', async () => {
  // The player who left in the last step is gone; the other three switch to a three-seat room.
  const three = [];
  for (const p of four) { const s = await st(p); if (s.screen === 'Game' && s.room === code4) three.push(p); }
  assert.equal(three.length, 3);
  await three[0].click('[data-act=to-lobby]');
  await Promise.all(three.map(p => until(p, () => window.__state().screen === 'Lobby' && window.__state().players === 3)));
  const host = three.includes(tess) ? tess : three[0];
  await host.selectOption('#rs-mode', 'ffa3');
  await Promise.all(three.map(p => until(p, () => document.querySelectorAll('.seats.four .seat').length === 3)));
  for (const p of three) { await p.waitForSelector('[data-act=ready]'); await p.click('[data-act=ready]'); }
  await Promise.all(three.map(p => until(p, () => window.__state().screen === 'Game' && window.__state().game.stage.startsWith('ChooseOrder'))));
  const seats = {};
  for (const p of three) seats[(await st(p)).game.me] = p;
  const chooser = Number((await st(three[0])).game.stage.match(/chooser: (\d)/)[1]);
  await seats[chooser].click('[data-act=setup-order][data-arg=first]');
  await Promise.all(three.map(p => until(p, () => window.__state().game.stage === 'ChooseBattlefield')));
  const first = (await st(three[0])).game.first;
  for (const [k, p] of Object.entries(seats)) if (Number(k) !== first) await p.click('.bfopt');
  await Promise.all(three.map(p => until(p, () => window.__state().game.stage === 'Mulligan')));
  assert.equal((await st(three[0])).game.nbf, 2);
  for (const p of three) { await p.waitForSelector('[data-act=mull-confirm]'); await p.click('[data-act=mull-confirm]'); }
  await Promise.all(three.map(p => until(p, () => window.__state().game.stage === 'Play')));
  assert.equal((await st(seats[first])).game.mode, 'Skirmish');
  assert.equal(await seats[first].$$eval('#g-others .plate.mini', e => e.length), 2);
  await seats[first].waitForTimeout(1200);
  await shot(seats[first], '48-table-ffa3');
  // "Show hand" puts the hand under the player's plate for both opponents; clicking opens it.
  await seats[first].click('[data-act=toggle-reveal-hand]');
  for (const k of [1, 2].map(k => (first + k) % 3)) {
    await until(seats[k], f => window.__state().game.visible_hands[f] === window.__state().game.hands[f], first);
    await seats[k].waitForFunction(() => [...document.querySelectorAll('#g-others .mini-hand .mh')].length === 4 && [...document.querySelectorAll('#g-others .mini-hand .mh')].every(i => i.complete && i.naturalWidth > 0), null, { timeout: 20000 });
    if (k === (first + 1) % 3) await shot(seats[k], '49a-ffa3-hand-strip');
    await seats[k].click('#g-others .mini-hand .mh');
    await seats[k].waitForSelector('.modal-bg .pk img');
    assert.equal(await seats[k].$$eval('.modal-bg .pk img', e => e.length), 4);
    await seats[k].waitForFunction(() => [...document.querySelectorAll('.modal-bg .pk img')].every(i => i.complete && i.naturalWidth > 0), null, { timeout: 20000 });
    if (k === (first + 1) % 3) await shot(seats[k], '49-ffa3-shown-hand');
    await seats[k].click('.modal-bg [data-act=close-modal] >> nth=-1');
  }
  // A showdown banner must not cover the other players' plates, on any screen size.
  const host3 = seats[first];
  const [bx, by] = await host3.evaluate(() => window.__zone({ BfCard: 0 }));
  await host3.mouse.click(bx, by);
  await host3.click('[data-act=bf-showdown]');
  await host3.waitForSelector('#g-banner .sd');
  const overlaps = () => {
    const b = document.querySelector('#g-banner .sd').getBoundingClientRect();
    return [...document.querySelectorAll('#g-others .plate.mini, #g-others .mini-hand')].some(e => { const r = e.getBoundingClientRect(); return r.left < b.right && b.left < r.right && r.top < b.bottom && b.top < r.bottom; });
  };
  for (const [w, h] of [[1600, 900], [1100, 800], [390, 844]]) {
    await host3.setViewportSize({ width: w, height: h });
    await host3.waitForTimeout(300);
    assert.equal(await host3.evaluate(overlaps), false, `showdown banner overlaps a plate at ${w}x${h}`);
    await shot(host3, `50-ffa3-showdown-${w}`);
  }
  await host3.setViewportSize({ width: 1600, height: 900 });
  await host3.click('[data-act=end-showdown]');
  const order = [1, 2].map(k => (first + k) % 3);
  await seats[order[0]].click('[data-act=game-menu]');
  await seats[order[0]].click('[data-act=concede]');
  await until(seats[first], o => { const g = window.__state().game; return g.conceded[o] && g.winner === null; }, order[0]);
  await seats[order[1]].click('[data-act=game-menu]');
  await seats[order[1]].click('[data-act=concede]');
  await seats[first].waitForSelector('.result.win');
});

await browser.close();
console.log(`\n${passed} steps passed, ${errors.length} problem(s)`);
for (const e of errors) console.log('  -', e);
process.exit(errors.length ? 1 : 0);
