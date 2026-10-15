import json,sys
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        b=json.loads(self.rfile.read(int(self.headers['content-length'])))
        json.dump(b,open('/home/claude/lab/captured_request.json','w'),ensure_ascii=False)
        d=json.dumps({"id":"x","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}).encode()
        self.send_response(200); self.send_header('content-type','application/json'); self.send_header('content-length',str(len(d))); self.end_headers(); self.wfile.write(d)
HTTPServer(('127.0.0.1',18081),H).handle_request()
