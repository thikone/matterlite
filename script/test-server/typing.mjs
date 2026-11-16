// SPDX-License-Identifier: AGPL-3.0-only

// Sends user_typing as the token's user to a channel every 2 s for the given seconds.
// Usage: MM_TOKEN=... node typing.mjs <channel_id> <seconds>
const [channel, seconds = '8'] = process.argv.slice(2);
const ws = new WebSocket('ws://localhost:8065/api/v4/websocket');
let seq = 1;
ws.onopen = () => {
  ws.send(JSON.stringify({seq: seq++, action: 'authentication_challenge', data: {token: process.env.MM_TOKEN}}));
  const send = () => ws.send(JSON.stringify({seq: seq++, action: 'user_typing', data: {channel_id: channel, parent_id: ''}}));
  setTimeout(send, 300);
  const timer = setInterval(send, 2000);
  setTimeout(() => { clearInterval(timer); ws.close(); }, Number(seconds) * 1000);
};
ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.seq_reply !== undefined) console.log('reply', message.seq_reply, message.status);
};
