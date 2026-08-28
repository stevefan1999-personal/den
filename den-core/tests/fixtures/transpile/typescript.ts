interface Greeter {
  name: string;
}
const greet = (who: Greeter): string => `hello ${who.name}`;
greet({ name: "den" } as Greeter);
