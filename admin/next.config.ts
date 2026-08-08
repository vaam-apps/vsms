import type { NextConfig } from "next";

const config: NextConfig = {
  reactStrictMode: true,
  transpilePackages: ["@vsms/env", "@vsms/ui"],
};

export default config;
