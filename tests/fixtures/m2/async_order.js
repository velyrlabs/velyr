console.log("a"); nativeAsync().then(v => console.log(v)); Promise.resolve().then(() => console.log("micro"));
