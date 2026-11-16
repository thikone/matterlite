// SPDX-License-Identifier: AGPL-3.0-only

// Prints the WebSocket events of the given types the token's user receives, for N seconds.
// Usage: MM_TOKEN=... node listen.mjs <seconds> <event> [<event> ...]
const [seconds = '10', ...types] = process.argv.slice(2);
const ws = new WebSocket('ws://localhost:8065/api/v4/websocket');
const start = Date.now();
ws.onopen = () => {
  ws.send(JSON.stringify({seq: 1, action: 'authentication_challenge', data: {token: process.env.MM_TOKEN}}));
  setTimeout(() => ws.close(), Number(seconds) * 1000);
};
ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.event && (types.length === 0 || types.includes(message.event))) {
    const at = ((Date.now() - start) / 1000).toFixed(1);
    console.log(at + 's', message.event, JSON.stringify(message.data).slice(0, 160));
  }
};
