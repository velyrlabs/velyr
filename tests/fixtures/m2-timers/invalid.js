try { queueMicrotask(1); } catch (e) { console.log(e instanceof TypeError); }
try { setTimeout("x"); } catch (e) { console.log(e instanceof TypeError); }
try { setInterval(null); } catch (e) { console.log(e instanceof TypeError); }
