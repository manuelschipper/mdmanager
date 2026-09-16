const UPSERT_INSTALLER_FETCH = `
  INSERT INTO installer_fetches (day, fetches)
  VALUES (?, 1)
  ON CONFLICT (day)
  DO UPDATE SET fetches = fetches + 1
`;

export async function onRequest(context) {
  const response = await context.next();

  if (
    context.request.method === "GET" &&
    new URL(context.request.url).pathname === "/install" &&
    response.ok
  ) {
    const day = new Date().toISOString().slice(0, 10);
    context.waitUntil(
      context.env.INSTALL_METRICS.prepare(UPSERT_INSTALLER_FETCH)
        .bind(day)
        .run()
        .catch((error) => console.error("failed to record installer fetch", error)),
    );
  }

  return response;
}
