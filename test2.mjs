const timer = setInterval(() => {
    console.log("hello world")
}, 100)

setTimeout(() => clearInterval(timer), 1000)

export const hello = "world"