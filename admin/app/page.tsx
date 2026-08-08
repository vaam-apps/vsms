export default function Home() {
  return (
    <main className="min-h-screen flex items-center justify-center bg-base-100">
      <div className="card bg-base-200 shadow-xl w-96">
        <div className="card-body">
          <h2 className="card-title">Scaffold Active</h2>
          <p>The Next.js 15 admin dashboard scaffold is working.</p>
          <p className="text-sm text-base-content/70">Tailwind 4 + daisyUI v5 styling is active.</p>
          <div className="card-actions justify-end">
            <button className="btn btn-primary">Get Started</button>
          </div>
        </div>
      </div>
    </main>
  );
}
