// Generates deck-code fixtures from the reference JS library for the Rust tests.
const { getCodeFromDeck } = require('@piltoverarchive/riftbound-deck-codes');
const fs = require('fs');
const decks = [
  { main: [{ cardCode: 'OGN-251', count: 1 }, { cardCode: 'OGN-001', count: 3 }, { cardCode: 'OGN-007', count: 6 }, { cardCode: 'SFD-120a', count: 2 }, { cardCode: 'OGN-276', count: 1 }], side: [{ cardCode: 'OGN-010', count: 2 }], champ: 'OGN-030' },
  { main: [{ cardCode: 'VEN-R01', count: 6 }, { cardCode: 'VEN-001', count: 3 }, { cardCode: 'UNL-229s', count: 1 }], side: [], champ: undefined },
  { main: [{ cardCode: 'VEN-SP1', count: 20 }, { cardCode: 'OGN-003', count: 2 }], side: [{ cardCode: 'OGN-004', count: 1 }], champ: 'OGN-030' },
  { main: [{ cardCode: 'OGN-100', count: 13 }], side: [], champ: undefined },
];
const out = decks.map(d => ({ code: getCodeFromDeck(d.main, d.side, d.champ), legends: [], ...d }));
out.push({ ...decks[0], legends: ['OGN-249', 'OGN-247'], code: getCodeFromDeck(decks[0].main, decks[0].side, decks[0].champ, ['OGN-249', 'OGN-247']) });
fs.writeFileSync('../shared/tests/deckcode_fixtures.json', JSON.stringify(out, null, 1));
console.log(out.map(o => o.code).join('\n'));
