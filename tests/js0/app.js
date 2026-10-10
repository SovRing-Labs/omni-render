// SPDX-License-Identifier: MIT OR Apache-2.0
const ul = document.querySelector('#list');
for (const w of ['alpha', 'beta', 'gamma']) { const li = document.createElement('li'); li.className = 'item'; li.textContent = w; ul.appendChild(li); }
