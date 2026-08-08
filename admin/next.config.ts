import type { NextConfig } from "next";

const config: NextConfig = {
  reactStrictMode: true,
  transpilePackages: ["@vsms/env", "@vsms/ui", "@vsms/gateway", "@vsms/api", "@vsms/hooks"],
};

export default config;
