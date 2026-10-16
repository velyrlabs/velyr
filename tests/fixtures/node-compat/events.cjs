const EventEmitter = require("events");
const emitter = new EventEmitter();
emitter.on("hi", (name) => console.log("hello", name));
emitter.emit("hi", "velyr");
