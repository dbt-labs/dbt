import { Link } from 'react-router-dom';

export default function NotFoundPage() {
  return (
    <div className="main-inner" style={{ padding: 32 }}>
      <h1>404</h1>
      <p className="muted">No page at this URL.</p>
      <Link to="/">Go home</Link>
    </div>
  );
}
