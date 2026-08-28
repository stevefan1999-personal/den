globalThis.React = {
  createElement: (tag, props, ...children) =>
    typeof tag === "function"
      ? tag({ ...props, children })
      : `<${tag}${props?.id ? ` id="${props.id}"` : ""}>${children.join("")}</${tag}>`,
};
const Item = ({ label }) => <li>{label}</li>;
String(<ul id="list"><Item label="den" /></ul>);
