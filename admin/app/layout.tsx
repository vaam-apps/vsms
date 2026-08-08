import type { Metadata } from 'next';
import './globals.css';

export const metadata: Metadata = {
  title: 'vsms Admin Console',
  description: 'A2P SMS gateway admin dashboard',
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" data-theme="dark">
      <body>{children}</body>
    </html>
  );
}
