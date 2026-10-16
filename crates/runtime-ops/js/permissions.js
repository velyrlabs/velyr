// Velyr.permissions.query on top of the private query op. Runs once per isolate, after bootstrap.js.
(function (global) {
  "use strict";

  const queryOp = global.op_permissions_query;
  delete global.op_permissions_query;

  // Descriptor field that carries the target of each permission name.
  const TARGET_FIELD = {
    read: "path",
    write: "path",
    native: "path",
    net: "host",
    listen: "host",
    env: "variable",
    run: "command",
  };

  function toQuery(descriptor) {
    if (descriptor === null || typeof descriptor !== "object") {
      throw new TypeError("Velyr.permissions.query expects a descriptor object");
    }
    const { name, port } = descriptor;
    // An import is remote when it names a host, local when it names a path.
    const kind = name === "import" ? (descriptor.host !== undefined ? "import-remote" : "import-local") : name;
    const field = kind === "import-remote" ? "host" : kind === "import-local" ? "path" : TARGET_FIELD[name];
    const target = field === undefined ? undefined : descriptor[field];
    if (target !== undefined && typeof target !== "string") {
      throw new TypeError("permission target `" + field + "` must be a string");
    }
    return [typeof name === "string" ? kind : String(name), target, port];
  }

  const permissions = {
    query: async (descriptor) => ({ state: queryOp(...toQuery(descriptor)) ? "granted" : "denied" }),
  };
  Object.defineProperty(global.Velyr, "permissions", {
    value: Object.freeze(permissions),
    writable: false,
    configurable: true,
    enumerable: true,
  });
})(globalThis);
