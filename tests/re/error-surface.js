// SPDX-License-Identifier: MIT OR Apache-2.0
// error-surface.js — caught errors expose name/message/constructor like node's.
// Our error objects shipped without `constructor`, so `catch (e) { e.constructor.name }` — a common
// rethrow/branch idiom — threw a second TypeError from inside the handler. Found by this harness.
const seen = [];
try { null.x; } catch (e) { seen.push(["name", e.name].join("=")); }
try { undefinedFn(); } catch (e) { seen.push(["name", e.name].join("=")); }
try { JSON.parse("{"); } catch (e) { seen.push(["name", e.name].join("=")); }
try { (1)(); } catch (e) { seen.push(["name", e.name].join("=")); }
for (const s of seen) console.log(s);

try { null.x; } catch (e) { console.log("ctor", e.constructor.name); }
try { null.x; } catch (e) { console.log("message", e.message); }
try { null.x; } catch (e) { console.log("typeof ctor", typeof e.constructor); }
try { null.x; } catch (e) { console.log("is Error", e instanceof Error); }

// nested throw through a function boundary keeps name and message
function inner() { throw new TypeError("deep"); }
try { inner(); } catch (e) { console.log("nested", e.name, e.message); }

// rethrow idiom that used to explode
try { try { null.x; } catch (e) { throw e; } } catch (e) { console.log("rethrown", e.constructor.name); }