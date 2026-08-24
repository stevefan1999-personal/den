use color_eyre::eyre;
use den_core::engine::Engine;
use rquickjs::FromJs;

async fn eval<T>(source: &str) -> eyre::Result<T>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    Ok(Engine::new().await.eval(source).await?)
}

async fn run(source: &str) -> eyre::Result<()> {
    let _: String = eval(&format!("{source}\n\"ok\"")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn crypto_random_uuid_has_the_version_4_shape() -> eyre::Result<()> {
    run(include_str!("js/uuid.js")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn crypto_get_random_values_fills_the_array_in_place() -> eyre::Result<()> {
    run(include_str!("js/random_values.js")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn crypto_subtle_digest_sha256_of_abc_matches_the_well_known_hex() -> eyre::Result<()> {
    run(include_str!("js/digest.js")).await
}

/// FIPS 180-4 / RFC 6234 §8.3, one block of `"abc"` for each digest
/// SubtleCrypto is required to implement.
#[tokio::test(flavor = "multi_thread")]
async fn digest_of_abc_matches_the_well_known_hexes() -> eyre::Result<()> {
    let failures: String = eval(
        r#"
          const hex = (buffer) => [...new Uint8Array(buffer)]
            .map((byte) => byte.toString(16).padStart(2, "0"))
            .join("");
          const abc = new TextEncoder().encode("abc");
          const known = {
            "SHA-1": "a9993e364706816aba3e25717850c26c9cd0d89d",
            "SHA-256":
              "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA-384":
              "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            "SHA-512":
              "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
          };
          const checks = {};
          for (const [name, expected] of Object.entries(known)) {
            const digest = await crypto.subtle.digest(name, abc);
            checks[name] = hex(digest) === expected;
            checks[`${name} is ArrayBuffer`] = digest instanceof ArrayBuffer;
          }
          Object.entries(checks)
            .filter(([, held]) => !held)
            .map(([name]) => name)
            .join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn digest_rejects_an_unknown_algorithm_with_not_supported_error() -> eyre::Result<()> {
    let report: String = eval(
        r#"
          const abc = new TextEncoder().encode("abc");
          let report = "no rejection";
          try {
            await crypto.subtle.digest("SHA-0", abc);
          } catch (error) {
            report = error instanceof DOMException ? error.name : `wrong: ${error}`;
          }
          report
        "#,
    )
    .await?;
    assert_eq!(report, "NotSupportedError");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn digest_accepts_algorithm_objects_and_buffer_source_views() -> eyre::Result<()> {
    let failures: String = eval(
        r#"
          const hex = (buffer) => [...new Uint8Array(buffer)]
            .map((byte) => byte.toString(16).padStart(2, "0"))
            .join("");
          const expected =
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
          const abc = new TextEncoder().encode("abc");
          const padded = new ArrayBuffer(5);
          new Uint8Array(padded).set([0, 97, 98, 99, 0]);
          const view = new DataView(padded, 1, 3);
          const checks = {
            lowerCase: hex(await crypto.subtle.digest("sha-256", abc)) === expected,
            algorithmObject:
              hex(await crypto.subtle.digest({ name: "SHA-256" }, abc)) === expected,
            dataView: hex(await crypto.subtle.digest("SHA-256", view)) === expected,
            arrayBuffer: hex(await crypto.subtle.digest("SHA-256", abc.buffer)) === expected,
          };
          Object.entries(checks)
            .filter(([, held]) => !held)
            .map(([name]) => name)
            .join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}
