// frontend/src/App.jsx
import React, { useState } from "react";
import * as Y from "yjs";
import { WebsocketProvider } from "y-websocket";
import axios from "axios"; // ← import axios
import Chat from "./Chat";

export default function App() {
  const [roomCode, setRoomCode] = useState("");
  const [userName, setUserName] = useState("");
  const [userId] = useState(() => Math.random().toString(36).slice(2));
  const [joined, setJoined] = useState(false);

  // Yjs state
  const [ydoc, setYdoc] = useState(null);
  const [yText, setYText] = useState(null);
  const [docContent, setDocContent] = useState("");

  const joinRoom = async (e) => {
    e.preventDefault();
    if (!roomCode || !userName) return;

    // 1) Yjs doc + WebSocket
    const doc = new Y.Doc();
    const provider = new WebsocketProvider(
      "ws://localhost:1234/ws",
      roomCode,
      doc
    );
    const text = doc.getText("document");
    try {
      const res = await axios.get(`http://localhost:1234/api/doc/${roomCode}`);
      const initial = res.data;
      if (initial) {
        doc.transact(() => {
          text.delete(0, text.length);
          text.insert(0, initial);
        });
        setDocContent(initial);
      }
    } catch (err) {
      // no existing doc → start empty
    }

    // 3) Initialize & subscribe
    setDocContent(text.toString());
    text.observe(() => {
      setDocContent(text.toString());
    });

    setYdoc(doc);
    setYText(text);
    setJoined(true);
  };

  const handleDocChange = async (e) => {
    const newValue = e.target.value;
    setDocContent(newValue);

    // 1) Update the CRDT
    if (yText && ydoc) {
      ydoc.transact(() => {
        yText.delete(0, yText.length);
        yText.insert(0, newValue);
      });
    }

    // 2) Persist via REST
    try {
      await axios.post("http://localhost:1234/api/update", {
        room: roomCode,
        content: newValue,
        timestamp: new Date().toISOString(),
      });
    } catch (err) {
      console.error("Failed to call /api/update:", err);
    }
  };

  if (!joined) {
    return (
      <div className="h-screen flex items-center justify-center bg-gray-100">
        <form
          onSubmit={joinRoom}
          className="bg-white p-6 rounded shadow-md space-y-4 w-80"
        >
          <h2 className="text-xl font-semibold">Join a Room</h2>
          <input
            className="w-full p-2 border rounded"
            placeholder="Room Code"
            value={roomCode}
            onChange={(e) => setRoomCode(e.target.value)}
          />
          <input
            className="w-full p-2 border rounded"
            placeholder="Your Name"
            value={userName}
            onChange={(e) => setUserName(e.target.value)}
          />
          <button
            type="submit"
            className="w-full bg-blue-600 text-white p-2 rounded hover:bg-blue-700"
          >
            Join
          </button>
        </form>
      </div>
    );
  }

  return (
    <div className="h-screen grid grid-cols-2">
      {/* Document editor */}
      <div className="p-4">
        <h3 className="mb-2 font-medium">Shared Document: {roomCode}</h3>
        <textarea
          className="w-full h-[90%] border p-2 rounded resize-none"
          value={docContent}
          onChange={handleDocChange} // ← updated handler
        />
      </div>

      {/* Chat sidebar */}
      <div className="p-4 border-l">
        <h3 className="mb-2 font-medium">Chat</h3>
        <Chat roomId={roomCode} userId={userId} userName={userName} />
      </div>
    </div>
  );
}
