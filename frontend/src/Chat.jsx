import React, { useEffect, useState } from "react";
import axios from "axios";

export default function Chat({ roomId, userId, userName }) {
  const [messages, setMessages] = useState([]);
  const [input, setInput] = useState("");

  useEffect(() => {
    axios
      .get(`/api/chat/${roomId}`)
      .then((res) => {
        console.log("chat API returned:", res.data);
        const data = res.data;
        setMessages(Array.isArray(data) ? data : []);
      })
      .catch((err) => {
        console.error("Failed to load chat history:", err);
        setMessages([]);
      });
  }, [roomId]);

  const sendMessage = async () => {
    const text = input.trim();
    if (!text) return;

    const msg = { roomId, userId, userName, message: text };
    try {
      await axios.post("/api/chat", msg);
      setMessages((prev) => [...prev, msg]);
      setInput("");
    } catch (err) {
      console.error("Failed to send message:", err);
    }
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex-1 overflow-y-auto p-2">
        {messages.length === 0 ? (
          <p className="text-gray-500 italic">No chat history yet.</p>
        ) : (
          messages.map((m, i) => (
            <div key={i} className="mb-1">
              <strong>{m.userName}:</strong> {m.message}
            </div>
          ))
        )}
      </div>
      <div className="p-2 border-t">
        <input
          className="w-full p-2 border rounded"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="Type a message..."
          onKeyDown={(e) => e.key === "Enter" && sendMessage()}
        />
      </div>
    </div>
  );
}
