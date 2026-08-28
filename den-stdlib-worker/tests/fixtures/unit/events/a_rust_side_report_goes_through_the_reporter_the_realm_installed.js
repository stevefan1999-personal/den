globalThis.seen = "nothing";
                       natives.reportException = (value) => {
                         globalThis.seen = `chain:${value.message}`;
                       };
