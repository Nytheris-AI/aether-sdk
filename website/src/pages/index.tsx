import type {ReactNode} from 'react';
import Head from '@docusaurus/Head';
import {LandingPage} from '@site/src/components/Landing/LandingPage';

export default function Home(): ReactNode {
  return (
    <>
      <Head>
        <html lang="en" />
        <title>AETHER — Unified AI Memory Layer</title>
        <meta
          name="description"
          content="Open-source Rust memory layer unifying episodic timelines, semantic knowledge graphs, and procedural rules across vector, embedded, cache, and graph stores."
        />
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link rel="preconnect" href="https://fonts.gstatic.com" crossOrigin="anonymous" />
        <link
          href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500&family=Space+Grotesk:wght@400;500;600;700&display=swap"
          rel="stylesheet"
        />
      </Head>
      <LandingPage />
    </>
  );
}
